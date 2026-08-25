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

    /// Close a session — the Z-report. A PRIVILEGED mutation: the manager PIN on
    /// [`NewClose::manager`] is verified server-side before anything is read or written, because a
    /// close is the one POS verb that books a GL correction on its own account.
    ///
    /// Order of operations (every guard runs before any write — a failed close leaves NOTHING):
    ///
    /// 1. **Guards.** The session must exist and be open; it must hold no DRAFT sale tickets
    ///    ([`PosError::SessionHasDraftOrders`] — post or void them first); and no half-recognised
    ///    tickets (billing linked but the draft→paid flip never landed —
    ///    [`PosError::SessionHasUnpostedInvoices`]; retry recognition first). The manager PIN is
    ///    verified. A second open session on the register is impossible by the one-open-session
    ///    unique, and opening a new one while this close is in flight collides there and surfaces as
    ///    [`PosError::SessionAlreadyOpen`].
    /// 2. **Count.** For each tender method `expected = opening_float + Σ recognised tenders` (cash
    ///    also `− Σ change_due + Σ cash-movement net`) — the statement legs (what the journals say)
    ///    against the counted legs (what the drawer holds). `difference = counted − expected`.
    /// 3. **Variance.** A non-zero net difference books through the `PosCashVariancePort` to the
    ///    register's cash account against its difference/write-off account — the ONE new GL surface a
    ///    close produces (per-ticket posting stays THE posting path). Booking happens BEFORE the
    ///    closing-entry transaction: if the seam refuses, the close rolls back entirely (nothing has
    ///    been written); the seam's idempotency key is the session, so a crash between booking and
    ///    commit re-books nothing on retry.
    /// 4. **Persist.** The closing entry (per-method breakdown + grand total + difference) and the
    ///    session's flip to `closed` commit as ONE unit, then `PosSessionClosed` is emitted.
    pub async fn close_session(
        &self,
        c: NewClose,
        variance: &dyn super::pos_ports::PosCashVariancePort,
    ) -> Result<CloseOutcome, PosError> {
        // RLS scope (ADR-0008): company on the DTO — bind it for the whole close, so the drawer reads
        // and the closing-entry transaction are all fenced.
        let company = c.company_id;
        company_scope::with_company_scope(Some(company), async move {
        // Guard: the session must exist and be open (this is also what makes close effectively-once —
        // a retried close of an already-closed session refuses here).
        let st = self.openings.fetch_status(&self.db_pool, c.opening_entry_id, c.company_id).await?;
        match st.as_deref() {
            None => return Err(PosError::SessionNotFound(c.opening_entry_id)),
            Some("open") => {}
            Some(_) => return Err(PosError::SessionNotOpen),
        }
        // Guard: no DRAFT sale tickets may remain — a draft has no GL and would orphan the drawer.
        let drafts = self.invoices.count_draft_orders_for_session(&self.db_pool, c.opening_entry_id).await?;
        if drafts > 0 {
            return Err(PosError::SessionHasDraftOrders(drafts));
        }
        // Guard: no half-recognised tickets — billing raised but the settle never landed. Closing
        // over them would strand a posted invoice against an unclosed drawer.
        let unposted = self.invoices.count_linked_unposted_for_session(&self.db_pool, c.opening_entry_id).await?;
        if unposted > 0 {
            return Err(PosError::SessionHasUnpostedInvoices(unposted));
        }
        // Guard: privileged — verify the manager's PIN (server-side, against the stored hash, with the
        // caller's source address feeding the per-address throttle ring).
        self.verify_manager_internal(c.company_id, &c.manager, c.source_ip.as_deref()).await?;

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
        let difference_total = money(difference_total);
        let totals_json = serde_json::Value::Array(by_method.iter().map(|r| serde_json::json!({
            "method": r.method, "expected": r.expected.to_string(), "counted": r.counted.to_string(), "difference": r.difference.to_string(),
        })).collect());

        // Variance booking (the one new GL surface at close): only a NON-ZERO difference books — a
        // balanced drawer raises no journal. Both the register's cash account and its write-off
        // account must be configured when the drawer did not balance.
        let mut booked: Option<super::pos_write_service::VarianceBooking> = None;
        if difference_total != Decimal::ZERO {
            let mut tx = self.db_pool.begin().await?;
            company_scope::bind_current_company(&mut tx).await?;
            let (cash_account, write_off_account, currency) = self
                .openings
                .fetch_variance_accounts_on(&mut tx, c.opening_entry_id)
                .await?;
            let closing_id = Uuid::new_v4();
            let cash_account = cash_account
                .ok_or(PosError::MissingAccount("cash_account_id (register settles a cash drawer)"))?;
            let write_off_account = write_off_account
                .ok_or(PosError::MissingAccount("write_off_account_id (register books drawer variance)"))?;
            let direction = if difference_total > Decimal::ZERO {
                super::pos_ports::CashVarianceDirection::Over
            } else {
                super::pos_ports::CashVarianceDirection::Short
            };
            let ack = variance
                .book_cash_variance(&super::pos_ports::CashVarianceRequest {
                    company_id: c.company_id,
                    opening_entry_id: c.opening_entry_id,
                    closing_entry_id: closing_id,
                    posting_date: c.closed_at.date(),
                    currency,
                    cash_account_id: cash_account,
                    difference_account_id: write_off_account,
                    amount: difference_total.abs(),
                    direction,
                })
                .await
                .map_err(|e| PosError::VarianceRejected { code: e.code, message: e.message })?;
            booked = Some(super::pos_write_service::VarianceBooking {
                journal_id: ack.journal_id,
                amount: difference_total.abs(),
                direction,
            });
            tx.commit().await?;
            // The closing entry below reuses this id so the booked journal traces to it.
            return self.finish_close(c, closing_id, totals_json, grand_total, invoice_count, difference_total, by_method, booked).await;
        }

        let id = Uuid::new_v4();
        self.finish_close(c, id, totals_json, grand_total, invoice_count, difference_total, by_method, booked).await
        }).await
    }

    /// The persist + emit tail of a close: closing entry + session flip in ONE transaction, then the
    /// `PosSessionClosed` event. Split out only so the variance-booked path can pass its pre-chosen
    /// closing id.
    async fn finish_close(
        &self,
        c: NewClose,
        closing_id: Uuid,
        totals_json: serde_json::Value,
        grand_total: Decimal,
        invoice_count: i64,
        difference_total: Decimal,
        by_method: Vec<MethodRecon>,
        booked: Option<super::pos_write_service::VarianceBooking>,
    ) -> Result<CloseOutcome, PosError> {
        let mut tx = self.db_pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let profile_id = self.openings.fetch_profile_id_on(&mut tx, c.opening_entry_id).await?;
        self.closings.insert_closing_entry(&mut tx, &NewClosingEntryRow {
            id: closing_id,
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
            closing_entry_id: closing_id, opening_entry_id: c.opening_entry_id, company_id: c.company_id,
            difference_total: money(difference_total),
        }));
        Ok(CloseOutcome { closing_id, difference_total: money(difference_total), by_method, variance: booked })
    }
}
