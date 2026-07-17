//! Assembling the printed receipt (hand-authored, user-owned).
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`] (the [`Receipt`]
//! type and its monospace `render_text` live there). Read-only: the GL is billing's, this is purely the
//! customer-facing slip.
//!
//! Per the module's 4-layer rule this file holds no SQL — the three reads live on
//! `PosInvoiceRepository` / `PosInvoiceItemRepository` / `PosPaymentRepository`.

use uuid::Uuid;

use super::pos_write_service::{PosError, PosWriteService, Receipt, ReceiptLine, ReceiptTender};

impl PosWriteService {
    /// Assemble the receipt for a ticket: register + money breakdown (incl. server-computed PPN) +
    /// lines + tenders + change. Tenant-scoped read — any ticket in the caller's company can be
    /// (re)printed. The GL is billing's; this is the customer-facing slip.
    pub async fn receipt(&self, company_id: Uuid, pos_invoice_id: Uuid) -> Result<Receipt, PosError> {
        // RLS scope (ADR-0008): read-only, company on the parameter.
        let hdr = self.invoices
            .fetch_receipt_header(&self.db_pool, pos_invoice_id, company_id).await?
            .ok_or(PosError::InvoiceNotFound(pos_invoice_id))?;

        let line_rows = self.items.fetch_receipt_lines(&self.db_pool, pos_invoice_id).await?;
        let lines = line_rows.into_iter().map(|r| {
            let desc = r.description.unwrap_or_else(|| r.item_id.to_string());
            ReceiptLine {
                description: desc, quantity: r.quantity, unit_price: r.unit_price,
                discount_amount: r.discount_amount, net_amount: r.net_amount,
            }
        }).collect();

        let tender_rows = self.payments.fetch_receipt_tenders(&self.db_pool, pos_invoice_id).await?;
        let tenders = tender_rows.into_iter().map(|r| ReceiptTender { method: r.method, amount: r.amount }).collect();

        Ok(Receipt {
            pos_invoice_id,
            receipt_number: hdr.receipt_number,
            register_name: hdr.register_name,
            posting_at: hdr.posting_at,
            currency: hdr.currency,
            status: hdr.status,
            lines,
            net_total: hdr.net_total,
            tax_rate: hdr.tax_rate,
            tax_total: hdr.tax_total,
            rounding_adjustment: hdr.rounding_adjustment,
            // The slip's TOTAL is the ROUNDED total — what the customer actually pays.
            grand_total: hdr.rounded_total,
            tenders,
            paid_total: hdr.paid_total,
            change_due: hdr.change_due,
        })
    }
}
