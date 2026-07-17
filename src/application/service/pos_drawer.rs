//! The cash drawer: non-sale movements, the X/Z read, and the close (hand-authored, user-owned).
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`]. `compute_drawer`
//! is the single expected-drawer calculation shared by `x_report` (mid-shift, read-only) and
//! `close_session` (the Z-report that counts and closes), so the two can never drift.
//!
//! Per the module's 4-layer rule this file holds no SQL — the statements live on
//! `PosOpeningEntryRepository` / `PosInvoiceRepository` / `PosPaymentRepository` /
//! `PosCashMovementRepository` / `PosClosingEntryRepository`. The closing-entry insert and the
//! session's flip to `closed` take THIS service's transaction, so a drawer is never counted without
//! the session closing.

use backbone_orm::company_scope;
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::infrastructure::persistence::{NewCashMovementRow, NewClosingEntryRow};

use super::pos_events::{PosEvent, PosSessionClosed};
use super::pos_write_service::{
    money, CloseOutcome, MethodExpected, MethodRecon, NewCashMovement, NewClose, PosError,
    PosWriteService, XReport,
};

impl PosWriteService {
    // ---- cash movements (non-sale drawer in/out) ---------------------------

    /// Record a non-sale cash drawer movement (`pay_in` / `pay_out` / `drop` / `no_sale`) against an
    /// OPEN session in the caller's tenant. `close_session` folds pay-ins into and pay-outs/drops out of
    /// the expected cash drawer, so a mid-shift movement no longer surfaces as an unexplained variance.
    pub async fn record_cash_movement(&self, m: NewCashMovement) -> Result<Uuid, PosError> {
        // Validate the kind + amount rule: no_sale carries no cash; the others are strictly positive.
        let amount = money(m.amount);
        match m.movement_type.as_str() {
            "no_sale" if amount != Decimal::ZERO => return Err(PosError::InvalidCashMovement("no_sale must have amount 0")),
            "pay_in" | "pay_out" | "drop" if amount <= Decimal::ZERO => return Err(PosError::InvalidCashMovement("amount must be positive")),
            "no_sale" | "pay_in" | "pay_out" | "drop" => {}
            _ => return Err(PosError::InvalidCashMovement("unknown movement_type")),
        }
        // RLS scope (ADR-0008): this method carries its company on the DTO, so bind it for the body —
        // the same pattern as `ring_sale`.
        let company = m.company_id;
        company_scope::with_company_scope(Some(company), async move {
            // Session must be OPEN and belong to the caller's tenant (same scope as ring_sale / close).
            let st = self.openings.fetch_status(&self.db_pool, m.opening_entry_id, m.company_id).await?;
            if st.as_deref() != Some("open") { return Err(PosError::SessionNotOpen); }

            let id = Uuid::new_v4();
            self.movements.insert_movement(&self.db_pool, &NewCashMovementRow {
                id,
                company_id: m.company_id,
                pos_profile_id: m.pos_profile_id,
                opening_entry_id: m.opening_entry_id,
                cashier_party_id: m.cashier_party_id,
                movement_type: &m.movement_type,
                amount,
                reason: m.reason.as_deref(),
                moved_at: m.moved_at,
            }).await?;
            Ok(id)
        }).await
    }

    // ---- drawer read (shared by close + X-report) --------------------------

    /// Compute the expected drawer for a session: per-method `expected = opening_float + Σ recognised
    /// tenders` (cash also `− Σ change_due + Σ cash-movement net`), plus the recognised grand total and
    /// paid-ticket count. Tenant-scoped, read-only — shared by `close_session` (Z-report) and
    /// `x_report` (mid-shift read) so the two can never drift.
    pub(super) async fn compute_drawer(&self, company_id: Uuid, opening_entry_id: Uuid) -> Result<(BTreeMap<String, Decimal>, Decimal, i64), PosError> {
        // RLS scope (ADR-0008): read-only, company on the parameter — bind it so every read below is
        // fenced (the caller may already be scoped; re-binding the same company is a no-op).
        let opening_json = self.openings
            .fetch_opening_balances(&self.db_pool, opening_entry_id, company_id).await?
            .ok_or(PosError::SessionNotFound(opening_entry_id))?;
        let mut expected: BTreeMap<String, Decimal> = BTreeMap::new();
        // A session with a NULL `opening_balances` (opened with no float) leaves `expected` empty —
        // only a MISSING session is an error, and the repo's outer `Option` carried that above.
        if let Some(sqlx::types::Json(serde_json::Value::Array(arr))) = opening_json {
            for e in arr {
                if let (Some(m), Some(a)) = (e.get("method").and_then(|v| v.as_str()), e.get("amount").and_then(|v| v.as_str())) {
                    *expected.entry(m.to_string()).or_insert(Decimal::ZERO) += Decimal::from_str_exact(a).unwrap_or(Decimal::ZERO);
                }
            }
        }
        // Σ tenders per method for recognised (paid) tickets in the session.
        let tender_rows = self.payments.sum_by_method_for_session(&self.db_pool, opening_entry_id).await?;
        for r in &tender_rows {
            *expected.entry(r.method.clone()).or_insert(Decimal::ZERO) += r.total;
        }
        // Cash change reduces the drawer.
        let cash_change = self.invoices.sum_change_due_for_session(&self.db_pool, opening_entry_id).await?;
        if cash_change > Decimal::ZERO {
            *expected.entry("cash".to_string()).or_insert(Decimal::ZERO) -= cash_change;
        }
        // Non-sale cash movements: pay-ins add to the drawer, pay-outs and drops remove from it
        // (no_sale has no cash effect). Folding the net in here means these no longer read as variance.
        let cash_movement_net = self.movements.net_for_session(&self.db_pool, opening_entry_id).await?;
        if cash_movement_net != Decimal::ZERO {
            *expected.entry("cash".to_string()).or_insert(Decimal::ZERO) += cash_movement_net;
        }
        let (grand_total, invoice_count) = self.invoices.session_totals(&self.db_pool, opening_entry_id).await?;
        Ok((expected, grand_total, invoice_count))
    }

    /// Mid-shift drawer read (X-report): the SAME expected drawer + running totals as the Z-report, but
    /// without counting, closing the session, or writing anything. Read-only + idempotent — a cashier
    /// can pull it any number of times during the shift. Requires an OPEN session in the caller's tenant.
    pub async fn x_report(&self, company_id: Uuid, opening_entry_id: Uuid) -> Result<XReport, PosError> {
        // RLS scope (ADR-0008): read-only, company on the parameter.
        company_scope::with_company_scope(Some(company_id), async move {
            let st = self.openings.fetch_status(&self.db_pool, opening_entry_id, company_id).await?;
            match st.as_deref() {
                None => return Err(PosError::SessionNotFound(opening_entry_id)),
                Some("open") => {}
                Some(_) => return Err(PosError::SessionNotOpen),
            }
            let (expected, grand_total, invoice_count) = self.compute_drawer(company_id, opening_entry_id).await?;
            let by_method = expected.into_iter().map(|(method, exp)| MethodExpected { method, expected: money(exp) }).collect();
            Ok(XReport { opening_entry_id, by_method, grand_total: money(grand_total), invoice_count })
        }).await
    }

    // ---- close (drawer reconciliation) -------------------------------------

    /// Close a session: for each tender method, `expected = opening_float + Σ recognised tenders`
    /// (cash also `− Σ change_due`); `difference = counted − expected`. Persist the per-method
    /// breakdown + the session's grand total, mark the session closed, emit `PosSessionClosed`.
    pub async fn close_session(&self, c: NewClose) -> Result<CloseOutcome, PosError> {
        // RLS scope (ADR-0008): company on the DTO — bind it for the whole close, so the drawer reads
        // and the closing-entry transaction are all fenced.
        let company = c.company_id;
        company_scope::with_company_scope(Some(company), async move {
        let (expected, grand_total, invoice_count) = self.compute_drawer(c.company_id, c.opening_entry_id).await?;

        let counted: BTreeMap<String, Decimal> = c.counted.iter().map(|(m, a)| (m.clone(), money(*a))).collect();
        let mut methods: std::collections::BTreeSet<String> = expected.keys().cloned().collect();
        methods.extend(counted.keys().cloned());
        let mut by_method = Vec::new();
        let mut difference_total = Decimal::ZERO;
        for m in methods {
            let exp = money(*expected.get(&m).unwrap_or(&Decimal::ZERO));
            let cnt = *counted.get(&m).unwrap_or(&Decimal::ZERO);
            let diff = cnt - exp;
            difference_total += diff;
            by_method.push(MethodRecon { method: m, expected: exp, counted: cnt, difference: diff });
        }
        let totals_json = serde_json::Value::Array(by_method.iter().map(|r| serde_json::json!({
            "method": r.method, "expected": r.expected.to_string(), "counted": r.counted.to_string(), "difference": r.difference.to_string(),
        })).collect());

        let id = Uuid::new_v4();
        let mut tx = self.db_pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let profile_id = self.openings.fetch_profile_id_on(&mut tx, c.opening_entry_id).await?;
        self.closings.insert_closing_entry(&mut tx, &NewClosingEntryRow {
            id,
            company_id: c.company_id,
            pos_profile_id: profile_id,
            opening_entry_id: c.opening_entry_id,
            closed_at: c.closed_at,
            cashier_party_id: c.cashier_party_id,
            totals_by_method: totals_json,
            grand_total: money(grand_total),
            invoice_count: invoice_count as i32,
            difference_total: money(difference_total),
        }).await?;
        self.openings.mark_closed(&mut tx, c.opening_entry_id).await?;
        tx.commit().await?;

        self.sink.publish(PosEvent::PosSessionClosed(PosSessionClosed {
            closing_entry_id: id, opening_entry_id: c.opening_entry_id, company_id: c.company_id,
            difference_total: money(difference_total),
        }));
        Ok(CloseOutcome { closing_id: id, difference_total: money(difference_total), by_method })
        }).await
    }
}
