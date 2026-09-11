//! Ringing a counter ticket (hand-authored, user-owned).
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`]: validate the
//! basket, derive the money through the shared compute core ([`super::pos_compute`] — document-grade
//! tax via the register's templates + register-config cash rounding), and write the ticket header +
//! its lines as ONE unit of work. `ring_sale_priced` is the promo cart seam (ADR-002) layered on top.
//!
//! Per the module's 4-layer rule this file holds no SQL — the statements live on
//! `PosInvoiceRepository` / `PosInvoiceItemRepository` / `PosProfileRepository` /
//! `PosOpeningEntryRepository`, and the header/line repo methods take THIS service's transaction so a
//! ticket is never half-rung.
//!
//! Tenancy is composition-installed (ADR-0029): the verb opens its own transaction, re-binds the
//! caller's ambient org scope onto it, and runs every validation read on that same connection — so
//! under a composed host the reads see through the fence exactly what the insert will write beside.

use rust_decimal::Decimal;
use uuid::Uuid;

use crate::infrastructure::persistence::NewDraftInvoiceRow;

use super::pos_compute::ComputeLineIn;
use super::pos_ports::PosTaxComputePort;
use super::pos_write_service::{
    legacy_company_echo, map_invoice_dup, relay_ambient_scope, PosError, PosWriteService, NewCartSale, NewSale,
    NewSaleLine,
};

impl PosWriteService {
    /// Ring a ticket. The money is SERVER-DERIVED end to end: the caller supplies the basket
    /// (items/qty/unit price/discount) and the register; the compute core resolves tax through the
    /// register's configured templates (`tax: &dyn PosTaxComputePort`) and rounds per the register's
    /// cash-rounding config. Nothing the caller might claim about totals is read.
    pub async fn ring_sale(&self, sale: NewSale, tax: &dyn PosTaxComputePort) -> Result<Uuid, PosError> {
        if sale.lines.is_empty() {
            return Err(PosError::EmptyDocument);
        }
        let mut tx = self.db_pool.begin().await?;
        relay_ambient_scope(&mut tx).await?;
        // Session must be open. Under a composed fence a session another unit owns reads as
        // plain absence — the refusal cannot distinguish whose it is, and must not.
        let st = self.openings.fetch_status_on(&mut *tx, sale.opening_entry_id).await?;
        if st.as_deref() != Some("open") {
            return Err(PosError::SessionNotOpen);
        }
        // ...and the drawer must be this ticket's register's: pairing register A's config with
        // register B's session would misattribute cash between tills.
        let session_profile = self
            .openings
            .fetch_profile_id_on(&mut *tx, sale.opening_entry_id)
            .await?
            .ok_or(PosError::SessionNotFound(sale.opening_entry_id))?;
        if session_profile != sale.pos_profile_id {
            return Err(PosError::SessionRegisterMismatch);
        }

        // Restaurant lane — seating. A named table must exist, and (the one draft per table
        // rule) no OTHER draft may already sit there: the pre-check gives the caller the
        // occupying ticket's id in the 409 body; the DB partial unique on (pos_table_id)
        // WHERE status='draft' is the race backstop (mapped below).
        if let Some(table) = sale.pos_table_id {
            if self.tables.fetch_table(&mut *tx, table).await?.is_none() {
                return Err(PosError::TableNotFound(table));
            }
            if let Some(occupant) = self.invoices.find_draft_on_table(&mut *tx, table, None).await? {
                return Err(PosError::TableOccupied { pos_table_id: table, draft_invoice_id: occupant });
            }
        }

        // Restaurant lane — order-level discount. Resolve the RATE from the master
        // (never the client), then fold it pro-rata into the line discounts BEFORE the compute,
        // so tax sees post-discount nets.
        let order_discount = self
            .resolve_order_discount(&mut tx, sale.pos_profile_id, sale.discount_id)
            .await?;

        // The one place a ticket's money is derived: templates -> document-grade tax -> register
        // cash rounding. A register with no configured templates refuses here (fail-closed).
        let mut compute_lines: Vec<ComputeLineIn> = sale
            .lines
            .iter()
            .map(|l| ComputeLineIn {
                item_id: l.item_id,
                revenue_account_id: l.revenue_account_id,
                description: l.description.clone(),
                course: l.course,
                quantity: l.quantity,
                unit_price: l.unit_price,
                discount_amount: l.discount_amount,
            })
            .collect();
        if let Some(d) = &order_discount {
            Self::fold_order_discount(&mut compute_lines, d.percentage)?;
        }
        let computed = self
            .compute_ticket(
                &mut *tx,
                sale.pos_profile_id,
                sale.posting_at.date(),
                super::pos_ports::PosTaxDocumentType::Invoice,
                compute_lines,
                tax,
            )
            .await?;
        let id = Uuid::new_v4();
        let r = self.invoices.insert_draft(&mut tx, &NewDraftInvoiceRow {
            id,
            client_uuid: None,
            pos_profile_id: sale.pos_profile_id,
            opening_entry_id: sale.opening_entry_id,
            branch_id: sale.branch_id,
            customer_id: sale.customer_id,
            pos_table_id: sale.pos_table_id,
            receipt_number: &sale.receipt_number,
            posting_at: sale.posting_at,
            net_total: computed.net_total,
            tax_total: computed.tax_total,
            grand_total: computed.grand_total,
            rounding_adjustment: computed.rounding_adjustment,
            rounded_total: computed.rounded_total,
        }).await;
        if let Err(e) = r {
            return Err(map_invoice_dup(
                e, &sale.receipt_number, None, sale.pos_table_id,
                &self.invoices, &self.db_pool, None,
            ).await);
        }
        for l in &computed.lines {
            self.items.insert_line(&mut tx, &crate::infrastructure::persistence::NewInvoiceItemRow {
                id: Uuid::new_v4(),
                pos_invoice_id: id,
                client_uuid: None,
                item_id: l.item_id,
                description: l.description.as_deref(),
                course: l.course,
                quantity: l.quantity,
                unit_price: l.unit_price,
                discount_amount: l.discount_amount,
                net_amount: l.net_amount,
                revenue_account_id: l.revenue_account_id,
            }).await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    /// Ring a ticket whose prices are resolved by promo's CART pricer (the cart seam, ADR-002). POS
    /// hands the whole basket (list prices + item dimensions + optional coupon) to the `CartPricingPort`;
    /// promo returns per-line nets folding in line rules, order-total discounts, and bundles. POS maps
    /// each net back to a `unit_price`/`discount_amount` pair so the compute core reproduces the
    /// conserved cart total. Zero normal Cargo edge to promo.
    pub async fn ring_sale_priced(
        &self,
        o: NewCartSale,
        pricing: &dyn crate::application::service::pos_cart_pricing::CartPricingPort,
        tax: &dyn PosTaxComputePort,
    ) -> Result<Uuid, PosError> {
        use crate::application::service::pos_cart_pricing::{CartPriceLine, CartPriceRequest};
        if o.lines.is_empty() {
            return Err(PosError::EmptyDocument);
        }
        let refs: Vec<Uuid> = o.lines.iter().map(|_| Uuid::new_v4()).collect();
        let req = CartPriceRequest {
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
            let gross = super::pos_write_service::money(pl.unit_price * l.quantity);
            let discount_amount = (gross - pl.net_line_total).max(Decimal::ZERO);
            lines.push(NewSaleLine {
                item_id: l.item_id,
                revenue_account_id: l.revenue_account_id,
                description: l.description.clone(),
                course: l.course,
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
                course: None,
                quantity: rl.quantity,
                unit_price: Decimal::ZERO,
                discount_amount: Decimal::ZERO,
            });
        }

        self.ring_sale(NewSale {
            pos_profile_id: o.pos_profile_id,
            opening_entry_id: o.opening_entry_id,
            branch_id: o.branch_id,
            customer_id: o.customer_id,
            pos_table_id: o.pos_table_id,
            discount_id: o.discount_id,
            receipt_number: o.receipt_number,
            posting_at: o.posting_at,
            lines,
        }, tax)
        .await
    }
}
