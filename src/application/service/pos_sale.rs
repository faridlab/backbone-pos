//! Ringing a counter ticket (hand-authored, user-owned).
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`]: validate the
//! basket, compute the money (server-owned PPN + IDR receipt rounding), and write the ticket header +
//! its lines as ONE unit of work. `ring_sale_priced` is the promo cart seam (ADR-002) layered on top.
//!
//! Per the module's 4-layer rule this file holds no SQL — the statements live on
//! `PosInvoiceRepository` / `PosInvoiceItemRepository` / `PosProfileRepository` /
//! `PosOpeningEntryRepository`, and the header/line repo methods take THIS service's transaction so a
//! ticket is never half-rung.

use backbone_orm::company_scope;
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::infrastructure::persistence::{NewDraftInvoiceRow, NewInvoiceItemRow};

use super::pos_write_service::{
    is_dup, money, round_to, NewCartSale, NewSale, NewSaleLine, PosError, PosWriteService,
};

impl PosWriteService {
    pub async fn ring_sale(&self, sale: NewSale) -> Result<Uuid, PosError> {
        // RLS scope (ADR-0008): the standalone lookups run through the scoped helpers (request
        // connection under HTTP), and the multi-row write runs in a transaction whose connection is
        // bound to this company via `bind_current_company` — so both the reads and the invoice/items
        // insert are fenced. Explicit `company_id` filters stay as defense-in-depth.
        let company = sale.company_id;
        company_scope::with_company_scope(Some(company), async move {
            if sale.lines.is_empty() { return Err(PosError::EmptyDocument); }
            // Session must be open AND belong to the caller's tenant.
            let st = self.openings.fetch_status(&self.db_pool, sale.opening_entry_id, sale.company_id).await?;
            if st.as_deref() != Some("open") { return Err(PosError::SessionNotOpen); }

            let mut net_total = Decimal::ZERO;
            let mut priced: Vec<(NewSaleLine, Decimal)> = Vec::with_capacity(sale.lines.len());
            for l in sale.lines {
                if l.quantity < Decimal::ZERO || l.unit_price < Decimal::ZERO || l.discount_amount < Decimal::ZERO {
                    return Err(PosError::NegativeAmount);
                }
                let net = money(l.quantity * l.unit_price) - money(l.discount_amount);
                if net < Decimal::ZERO { return Err(PosError::NegativeAmount); }
                net_total += net;
                priced.push((l, net));
            }
            let net_total = money(net_total);
            // PPN is server-owned: compute it from the register's configured rate (0 for a non-PKP till).
            // The `sale.tax_total` input is ignored — a merchant can neither omit nor overstate the VAT.
            let tax_rate: Decimal = self.profiles
                .fetch_tax_rate(&self.db_pool, sale.pos_profile_id, sale.company_id).await?
                .ok_or(PosError::ProfileNotFound(sale.pos_profile_id))?;
            let tax_total = money(net_total * tax_rate);
            let grand = net_total + tax_total;
            let rounded = round_to(grand, sale.round_to.unwrap_or(Decimal::ZERO));
            let rounding_adjustment = rounded - grand;
            let id = Uuid::new_v4();
            let mut tx = self.db_pool.begin().await?;
            company_scope::bind_current_company(&mut tx).await?;
            let r = self.invoices.insert_draft(&mut tx, &NewDraftInvoiceRow {
                id,
                company_id: sale.company_id,
                pos_profile_id: sale.pos_profile_id,
                opening_entry_id: sale.opening_entry_id,
                branch_id: sale.branch_id,
                customer_id: sale.customer_id,
                receipt_number: &sale.receipt_number,
                posting_at: sale.posting_at,
                net_total,
                tax_total,
                grand_total: grand,
                rounding_adjustment,
                rounded_total: rounded,
            }).await;
            if let Err(e) = r {
                return Err(if is_dup(&e) { PosError::DuplicateNumber(sale.receipt_number) } else { e.into() });
            }
            for (l, net) in &priced {
                self.items.insert_line(&mut tx, &NewInvoiceItemRow {
                    id: Uuid::new_v4(),
                    company_id: sale.company_id,
                    pos_invoice_id: id,
                    item_id: l.item_id,
                    description: l.description.as_deref(),
                    quantity: l.quantity,
                    unit_price: l.unit_price,
                    discount_amount: money(l.discount_amount),
                    net_amount: *net,
                    revenue_account_id: l.revenue_account_id,
                }).await?;
            }
            tx.commit().await?;
            Ok(id)
        }).await
    }

    /// Ring a ticket whose prices are resolved by promo's CART pricer (the cart seam, ADR-002). POS
    /// hands the whole basket (list prices + item dimensions + optional coupon) to the `CartPricingPort`;
    /// promo returns per-line nets folding in line rules, order-total discounts, and bundles. POS maps
    /// each net back to a `unit_price`/`discount_amount` pair so `ring_sale`'s own math reproduces the
    /// conserved cart total. Zero normal Cargo edge to promo.
    pub async fn ring_sale_priced(
        &self,
        o: NewCartSale,
        pricing: &dyn crate::application::service::pos_cart_pricing::CartPricingPort,
    ) -> Result<Uuid, PosError> {
        use crate::application::service::pos_cart_pricing::{CartPriceLine, CartPriceRequest};
        if o.lines.is_empty() {
            return Err(PosError::EmptyDocument);
        }
        let refs: Vec<Uuid> = o.lines.iter().map(|_| Uuid::new_v4()).collect();
        let req = CartPriceRequest {
            company_id: o.company_id,
            customer_id: o.customer_id,
            customer_group_id: o.customer_group_id,
            coupon_code: o.coupon_code.clone(),
            lines: o
                .lines
                .iter()
                .zip(&refs)
                .map(|(l, r)| CartPriceLine {
                    line_ref: *r,
                    item_id: l.item_id,
                    item_group_id: l.item_group_id,
                    brand_id: l.brand_id,
                    list_price: l.list_price,
                    quantity: l.quantity,
                })
                .collect(),
        };
        let priced = pricing
            .price_cart(&req)
            .await
            .map_err(|e| PosError::PricingRejected { code: e.code, message: e.message })?;

        let mut lines = Vec::with_capacity(o.lines.len());
        for (l, r) in o.lines.iter().zip(&refs) {
            let pl = priced
                .lines
                .iter()
                .find(|p| p.line_ref == *r)
                .ok_or_else(|| PosError::PricingRejected {
                    code: "pricing_line_missing".into(),
                    message: "pricer omitted a line".into(),
                })?;
            let gross = money(pl.unit_price * l.quantity);
            let discount_amount = (gross - pl.net_line_total).max(Decimal::ZERO);
            lines.push(NewSaleLine {
                item_id: l.item_id,
                revenue_account_id: l.revenue_account_id,
                description: l.description.clone(),
                quantity: l.quantity,
                unit_price: pl.unit_price,
                discount_amount,
            });
        }
        // Buy-X-get-Y: ring the free goods as zero-priced lines (they don't change the ticket total).
        for rl in &priced.reward_lines {
            lines.push(NewSaleLine {
                item_id: rl.item_id,
                revenue_account_id: None,
                description: Some("promo reward (free)".into()),
                quantity: rl.quantity,
                unit_price: Decimal::ZERO,
                discount_amount: Decimal::ZERO,
            });
        }

        self.ring_sale(NewSale {
            company_id: o.company_id,
            pos_profile_id: o.pos_profile_id,
            opening_entry_id: o.opening_entry_id,
            branch_id: o.branch_id,
            customer_id: o.customer_id,
            receipt_number: o.receipt_number,
            posting_at: o.posting_at,
            lines,
            tax_total: o.tax_total,
            round_to: o.round_to,
        })
        .await
    }
}
