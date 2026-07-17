//! The retail seam: recognising and returning a sale (hand-authored, user-owned).
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`]. POS posts no
//! GL itself — it drives `backbone-billing` and `backbone-payment` through their ports, and both
//! directions net cleanly: recognise books `Dr A/R · Cr Revenue` then `Dr Cash · Cr A/R`; return
//! reverses both. Every state flip is gated on its own `UPDATE ... WHERE status=<prior>` so a retry
//! recognises/returns at most once.
//!
//! Per the module's 4-layer rule this file holds no SQL — the statements live on
//! `PosInvoiceRepository` / `PosInvoiceItemRepository`. These paths are ID-only (no company argument),
//! so their `with_company_scope` contract is documented on each method and on the repo methods they
//! call.

use rust_decimal::Decimal;
use uuid::Uuid;

use crate::infrastructure::persistence::NewReturnInvoiceRow;

use super::pos_events::{PosEvent, PosInvoicePaid, PosInvoiceReturned};
use super::pos_ports::{
    BillingPort, CreditNoteRequest, InventoryPort, PaymentPort, RefundRequest, SaleInvoiceRequest,
    SaleLine, SettlementRequest, StockIssueLine, StockIssueRequest,
};
use super::pos_write_service::{PosError, PosWriteService, RecognizeOutcome, ReturnOutcome};

impl PosWriteService {
    /// Recognise a fully-tendered sale: drive billing (raise + post the Sales Invoice → revenue) then
    /// payment (settle the `rounded_total` against it → `Dr Cash · Cr A/R`), link the results, mark the
    /// ticket `paid`, and emit `PosInvoicePaid`. Idempotent on `status='paid'`.
    pub async fn recognize_sale(&self, pos_invoice_id: Uuid, billing: &dyn BillingPort, payment: &dyn PaymentPort, inventory: Option<&dyn InventoryPort>) -> Result<RecognizeOutcome, PosError> {
        // Short-circuit if already recognised.
        if let Some(o) = self.short_circuit_paid(pos_invoice_id).await? { return Ok(o); }
        // RLS scope (ADR-0008), ID-only pattern: identified by ticket id alone. Under HTTP the
        // request-dedicated connection carries the scope. When driven by an EVENT (the recognition
        // sink subscribing to PosTenderCompleted), the caller must wrap this in
        // `with_company_scope(Some(event.company_id))` — the event carries the company — otherwise
        // these reads fail closed.
        let inv = self.invoices
            .fetch_for_recognition(&self.db_pool, pos_invoice_id).await?
            .ok_or(PosError::InvoiceNotFound(pos_invoice_id))?;

        let rounded_total = inv.rounded_total;
        let paid_total = inv.paid_total;
        if paid_total < rounded_total {
            return Err(PosError::NotFullyTendered { rounded_total, paid_total });
        }
        let tax_total = inv.tax_total;
        let tax_account_id = inv.tax_account_id;
        let tax_rate = inv.tax_rate;
        // A ticket carrying PPN must have a tax liability account on its register to credit it to.
        if tax_total > Decimal::ZERO && tax_account_id.is_none() {
            return Err(PosError::MissingAccount("tax_account_id (register charges PPN but has no tax account)"));
        }
        let company_id = inv.company_id;
        let currency = inv.currency.clone();
        let receivable: Uuid = inv.receivable_account_id.ok_or(PosError::MissingAccount("receivable_account_id"))?;
        let income: Option<Uuid> = inv.income_account_id;
        let cash: Uuid = inv.cash_account_id.ok_or(PosError::MissingAccount("cash_account_id"))?;
        let customer: Uuid = inv.customer_id
            .or(inv.default_customer_id)
            .ok_or(PosError::MissingAccount("customer_id (no ticket or profile default)"))?;

        let posting_at = inv.posting_at;
        let posting_date = posting_at.date_naive();

        // Billing at-most-once (council 2026-07-05): raise the Sales Invoice only if it was not already
        // raised on a prior attempt. Persisting `billing_invoice_id` on the still-draft ticket the
        // instant `raise_and_post` returns makes a retry after a settle failure (declined card / closed
        // period) REUSE the invoice instead of raising a second one — otherwise the counter's ordinary
        // decline-then-retry doubles the revenue journal (billing does not dedup on the POS ticket).
        let existing_billing: Option<Uuid> = inv.billing_invoice_id;
        let invoice_id = if let Some(id) = existing_billing {
            id
        } else {
            // Sale lines → invoice request (revenue = line account, else profile income).
            let item_rows = self.items.fetch_revenue_lines(&self.db_pool, pos_invoice_id).await?;
            let mut lines = Vec::with_capacity(item_rows.len());
            for r in &item_rows {
                let rev = r.revenue_account_id.or(income).ok_or(PosError::MissingAccount("revenue account (line or profile income)"))?;
                // Pass the NET as the unit_price at qty 1 so billing's line net == POS net (discounts already applied).
                lines.push(SaleLine { item_id: r.item_id, revenue_account_id: rev, quantity: Decimal::ONE, unit_price: r.net_amount });
            }
            let ack = billing.raise_and_post(&SaleInvoiceRequest {
                company_id, customer_id: customer, currency: currency.clone(), source_pos_id: pos_invoice_id,
                receivable_account_id: receivable, posting_date, lines, tax_total, tax_account_id, tax_rate,
            }).await.map_err(|e| PosError::BillingRejected { code: e.code, message: e.message })?;
            // Persist the link WHILE STILL DRAFT — before settle — so any retry reuses this invoice.
            self.invoices.link_billing_invoice(&self.db_pool, pos_invoice_id, ack.invoice_id).await?;
            ack.invoice_id
        };

        // Settle the rounded_total against the raised invoice (cash sale → A/R nets to zero).
        let sack = payment.settle(&SettlementRequest {
            company_id, customer_id: customer, currency, invoice_ref: invoice_id,
            bank_account_id: cash, party_account_id: receivable, posting_date, amount: rounded_total,
        }).await.map_err(|e| PosError::PaymentRejected { code: e.code, message: e.message })?;

        // Gate the event on THIS invocation performing the draft→paid transition. Persist the settling
        // `payment_entry_id` in the same flip so a later return refunds the tender from the ticket (no
        // cross-schema lookup) and a crash-retry has a durable skip-gate (ADR-001 `settle`-idempotency).
        let rows_affected = self.invoices.mark_paid(&self.db_pool, pos_invoice_id, invoice_id, sack.payment_id).await?;
        if rows_affected == 1 {
            self.sink.publish(PosEvent::PosInvoicePaid(PosInvoicePaid {
                pos_invoice_id, company_id, grand_total: inv.grand_total, rounded_total,
                billing_invoice_id: invoice_id, payment_id: sack.payment_id,
            }));
            // Decrement stock (an outward Delivery Note: on-hand relieved, `Dr COGS · Cr Inventory`)
            // when the register is stock-tracking. Gated on this once-only flip, so it issues exactly
            // once even if recognise is retried. Non-stock registers (no warehouse) skip it.
            if let (Some(inv_port), Some(warehouse), Some(cogs), Some(inv_acct)) = (
                inventory,
                inv.warehouse_id,
                inv.cogs_account_id,
                inv.inventory_account_id,
            ) {
                let qty_rows = self.items.fetch_quantity_lines(&self.db_pool, pos_invoice_id).await?;
                let lines: Vec<StockIssueLine> = qty_rows.iter()
                    .map(|r| StockIssueLine { item_id: r.item_id, quantity: r.quantity })
                    .filter(|l| l.quantity > Decimal::ZERO)
                    .collect();
                if !lines.is_empty() {
                    inv_port.issue(&StockIssueRequest {
                        company_id, branch_id: inv.branch_id, customer_id: customer,
                        source_pos_id: pos_invoice_id, warehouse_id: warehouse, cogs_account_id: cogs,
                        inventory_account_id: inv_acct, posting_date, lines,
                    }).await.map_err(|e| PosError::InventoryRejected { code: e.code, message: e.message })?;
                }
            }
        }
        Ok(RecognizeOutcome { pos_invoice_id, billing_invoice_id: invoice_id, payment_id: sack.payment_id })
    }

    async fn short_circuit_paid(&self, pos_invoice_id: Uuid) -> Result<Option<RecognizeOutcome>, PosError> {
        let row = self.invoices
            .fetch_paid_state(&self.db_pool, pos_invoice_id).await?
            .ok_or(PosError::InvoiceNotFound(pos_invoice_id))?;
        if row.status == "paid" {
            if let Some(bid) = row.billing_invoice_id {
                // Return the real persisted payment_id on replay (was fabricated as Uuid::nil() before
                // payment_entry_id was persisted — nil only for pre-2026-07-14 legacy tickets).
                let payment_id = row.payment_entry_id.unwrap_or(Uuid::nil());
                return Ok(Some(RecognizeOutcome { pos_invoice_id, billing_invoice_id: bid, payment_id }));
            }
        }
        Ok(None)
    }

    /// Return a recognised sale (council 2026-07-05, the KEEP returns flow). Drives billing (credit
    /// note — `Dr Revenue · Cr A/R`, invoice → cancelled) and payment (refund — `Dr A/R · Cr Cash`),
    /// so revenue, cash, and A/R all net back to zero. Records an `is_return` ticket linked via
    /// `return_against`, flips the original → `returned`, emits `PosInvoiceReturned`. **Idempotent:**
    /// both downstream reversals are idempotent, and the ticket/event are gated on the original's
    /// `paid→returned` transition — a repeat return refunds/credits at most once. POS posts no GL.
    pub async fn return_sale(&self, original_pos_invoice_id: Uuid, billing: &dyn BillingPort, payment: &dyn PaymentPort) -> Result<ReturnOutcome, PosError> {
        // RLS scope (ADR-0008), ID-only pattern — see `add_tender`: the lookup is fenced by the
        // request-dedicated connection, so another company's ticket is simply not found.
        let o = self.invoices
            .fetch_return_source(&self.db_pool, original_pos_invoice_id).await?
            .ok_or(PosError::InvoiceNotFound(original_pos_invoice_id))?;
        let status = o.status.clone();
        if status != "paid" && status != "returned" {
            return Err(PosError::NotReturnable(status));
        }
        let billing_invoice_id: Uuid = o.billing_invoice_id.ok_or(PosError::NotReturnable("no billing invoice".into()))?;
        let company_id = o.company_id;
        let rounded_total = o.rounded_total;
        // The tender to reverse — persisted at recognition (nil only for pre-2026-07-14 legacy tickets).
        let payment_id: Uuid = o.payment_entry_id.unwrap_or(Uuid::nil());

        // Drive the two reversals (both idempotent downstream): refund the tender + credit-note the sale.
        payment.refund(&RefundRequest { company_id, invoice_ref: billing_invoice_id, payment_id, amount: rounded_total })
            .await.map_err(|e| PosError::PaymentRejected { code: e.code, message: e.message })?;
        billing.credit_note(&CreditNoteRequest { company_id, invoice_ref: billing_invoice_id })
            .await.map_err(|e| PosError::BillingRejected { code: e.code, message: e.message })?;

        // Gate the return ticket + event on the original's paid→returned transition (exactly-once).
        let rows_affected = self.invoices.mark_returned(&self.db_pool, original_pos_invoice_id).await?;
        let return_ticket_id = if rows_affected == 1 {
            let rt = Uuid::new_v4();
            self.invoices.insert_return_ticket(&self.db_pool, &NewReturnInvoiceRow {
                id: rt,
                company_id,
                pos_profile_id: o.pos_profile_id,
                opening_entry_id: o.opening_entry_id,
                branch_id: o.branch_id,
                customer_id: o.customer_id,
                receipt_number: format!("RET-{}", &original_pos_invoice_id.simple().to_string()[..12]),
                posting_at: o.posting_at,
                net_total: o.net_total,
                tax_total: o.tax_total,
                grand_total: o.grand_total,
                rounded_total,
                billing_invoice_id,
                return_against: original_pos_invoice_id,
            }).await?;
            self.sink.publish(PosEvent::PosInvoiceReturned(PosInvoiceReturned {
                pos_invoice_id: original_pos_invoice_id, return_ticket_id: rt, company_id,
                billing_invoice_id, amount: rounded_total,
            }));
            rt
        } else {
            // Already returned — reuse the existing return ticket (idempotent replay).
            self.invoices
                .find_return_ticket(&self.db_pool, original_pos_invoice_id).await?
                .unwrap_or(original_pos_invoice_id)
        };
        Ok(ReturnOutcome { pos_invoice_id: original_pos_invoice_id, return_ticket_id, billing_invoice_id })
    }
}
