//! Taking payment at the counter (hand-authored, user-owned).
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`]: add a tender
//! line, recompute the ticket's `paid_total`/`change_due` in the SAME transaction, and emit
//! `PosTenderCompleted` exactly on the tender that crosses full payment.
//!
//! Per the module's 4-layer rule this file holds no SQL — the statements live on
//! `PosPaymentRepository` / `PosInvoiceRepository`, and the tender-insert + re-sum + header-update repo
//! methods take THIS service's transaction so the tender and the totals it implies commit together.

use backbone_orm::company_scope;
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::infrastructure::persistence::NewTenderRow;

use super::pos_events::{PosEvent, PosTenderCompleted};
use super::pos_write_service::{money, PosError, PosWriteService, TenderOutcome};

impl PosWriteService {
    /// Add a tender line; recompute `paid_total` + `change_due` (overpayment). Ticket must be draft.
    pub async fn add_tender(&self, pos_invoice_id: Uuid, method: &str, amount: Decimal, reference_no: Option<String>) -> Result<TenderOutcome, PosError> {
        if amount <= Decimal::ZERO { return Err(PosError::NegativeAmount); }
        // RLS scope (ADR-0008), ID-only pattern: this method is identified by the ticket id alone —
        // there is no company argument to scope from up front. The header read therefore runs on the
        // REQUEST-dedicated connection (established by `company_auth`), which carries the caller's
        // `app.company_id`; RLS fences the lookup so another company's ticket simply isn't found.
        // Having read the ticket, we then bind its company onto our own transaction below.
        let hdr = self.invoices
            .fetch_tender_header(&self.db_pool, pos_invoice_id).await?
            .ok_or(PosError::InvoiceNotFound(pos_invoice_id))?;
        if hdr.status != "draft" { return Err(PosError::NotDraft); }
        let rounded_total = hdr.rounded_total;
        let hdr_company = hdr.company_id;
        let (paid_total, change_due) = company_scope::with_company_scope(Some(hdr_company), async move {
            let mut tx = self.db_pool.begin().await?;
            company_scope::bind_current_company(&mut tx).await?;
            self.payments.insert_tender(&mut tx, &NewTenderRow {
                id: Uuid::new_v4(),
                company_id: hdr_company,
                pos_invoice_id,
                payment_method: method,
                amount: money(amount),
                reference_no: reference_no.as_deref(),
            }).await?;
            let paid_total = self.payments.sum_paid_total_on(&mut tx, pos_invoice_id).await?;
            let change_due = if paid_total > rounded_total { paid_total - rounded_total } else { Decimal::ZERO };
            self.invoices.update_tender_totals(&mut tx, pos_invoice_id, paid_total, change_due).await?;
            // Durable event: if this tender crosses full payment AND an outbox schema is configured,
            // stage PosTenderCompleted INSIDE this transaction (atomic with the tender). A relay
            // drains it to recognition — surviving a crash between commit and the in-process spawn.
            // The fire-and-forget `sink.publish` below remains the fast path; double-delivery is a
            // no-op because recognition is replay-safe (billing reuses billing_invoice_id; settle
            // carries a payment_id skip-gate). Unset schema → historical fire-and-forget only.
            let fully_tendered = paid_total >= rounded_total;
            if fully_tendered && (paid_total - money(amount)) < rounded_total {
                if let Some(schema) = &self.outbox_schema {
                    // Outbox fenced by `company_id` (ADR-0011, mirrored from backbone-payment) — see
                    // `tender_completed_outbox_record` for the fence contract.
                    let rec = tender_completed_outbox_record(pos_invoice_id, hdr_company, chrono::Utc::now());
                    backbone_outbox::outbox::stage(&mut *tx, schema, &rec)
                        .await
                        .map_err(|e| PosError::Db(sqlx::Error::Protocol(e.to_string())))?;
                }
            }
            tx.commit().await?;
            Ok::<_, PosError>((paid_total, change_due))
        }).await?;
        // Emit PosTenderCompleted exactly on the tender that crosses full payment, so a subscriber can
        // recognise the sale. Guarding on the crossing (prev < total <= now) avoids a re-emit on any
        // extra tender added before recognition flips the ticket to paid.
        let fully_tendered = paid_total >= rounded_total;
        if fully_tendered && (paid_total - money(amount)) < rounded_total {
            self.sink.publish(PosEvent::PosTenderCompleted(PosTenderCompleted {
                pos_invoice_id, company_id: hdr_company,
            }));
        }
        Ok(TenderOutcome { paid_total, change_due, fully_tendered })
    }
}

/// The fenced outbox record for a just-completed tender (ADR-0011, mirrored from backbone-payment).
///
/// `company_id` is the owning tenant. backbone-outbox v2.7.4's `multi_tenant` feature fences
/// `<schema>.outbox_events` by it — a backfilled `company_id` column + a fail-closed RLS policy —
/// so a tenant-scoped relay cannot read another company's staged event. `OutboxRecord::new` sets the
/// top-level field; the payload keeps a copy for consumers that read it from the JSON.
fn tender_completed_outbox_record(
    pos_invoice_id: Uuid,
    company_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> backbone_outbox::OutboxRecord {
    let payload = serde_json::json!({
        "pos_invoice_id": pos_invoice_id,
        "company_id": company_id,
    });
    backbone_outbox::OutboxRecord::new(
        "PosTenderCompleted",
        "PosSale",
        pos_invoice_id.to_string(),
        company_id,
        payload,
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard for the ADR-0011 outbox fence (mirrored from backbone-payment): the staged
    /// `PosTenderCompleted` record MUST carry the owning tenant as its top-level `company_id` — the
    /// column the RLS policy fences on. If someone reverts to the pre-fence struct literal
    /// (company_id only in the payload, outbox v2.7.1), this fails.
    #[test]
    fn tender_completed_record_is_fenced_by_company() {
        let company = Uuid::new_v4();
        let ticket = Uuid::new_v4();
        let rec = tender_completed_outbox_record(ticket, company, chrono::Utc::now());
        assert_eq!(
            rec.company_id, company,
            "PosTenderCompleted outbox record must carry the owning tenant (ADR-0011 fence)"
        );
        assert_eq!(rec.event_type, "PosTenderCompleted");
        assert_eq!(rec.aggregate_type, "PosSale");
        assert_eq!(rec.aggregate_id, ticket.to_string());
    }
}
