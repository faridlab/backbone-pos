//! Validated write path + retail orchestrator for POS (hand-authored, user-owned).
//!
//! POS owns the cashier session + the counter ticket, but posts NO GL: on `recognize_sale` it drives
//! `backbone-billing` (raise + post the real Sales Invoice — revenue) and `backbone-payment` (settle
//! the tender) through the `BillingPort`/`PaymentPort`, so the retail path reuses the same GL emitters
//! as web/B2B sales. A cash sale therefore books `Dr A/R · Cr Revenue` (billing) then `Dr Cash · Cr
//! A/R` (payment) — A/R nets to zero at the counter. Close reconciles the drawer per tender method.

use rust_decimal::{Decimal, RoundingStrategy};
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

use super::pos_events::{PosEvent, PosEventSink, PosInvoicePaid, PosInvoiceReturned, PosSessionClosed, PosSessionOpened, LoggingSink};
use super::pos_ports::{BillingPort, CreditNoteRequest, PaymentPort, RefundRequest, SaleInvoiceRequest, SaleLine, SettlementRequest};

fn money(v: Decimal) -> Decimal {
    v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}
/// Round `v` to the nearest `step` (IDR receipt rounding; `step == 0` means no rounding).
fn round_to(v: Decimal, step: Decimal) -> Decimal {
    if step <= Decimal::ZERO { return money(v); }
    let n = (v / step).round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero);
    money(n * step)
}

// --- input structs -----------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NewSession {
    pub company_id: Uuid,
    pub pos_profile_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub cashier_party_id: Uuid,
    pub opened_at: chrono::NaiveDateTime,
    /// Opening float per method — e.g. [("cash", 500000)].
    pub opening_balances: Vec<(String, Decimal)>,
}

#[derive(Debug, Clone)]
pub struct NewSaleLine {
    pub item_id: Uuid,
    pub revenue_account_id: Option<Uuid>,
    pub description: Option<String>,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    pub discount_amount: Decimal,
}

#[derive(Debug, Clone)]
pub struct NewSale {
    pub company_id: Uuid,
    pub pos_profile_id: Uuid,
    pub opening_entry_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub receipt_number: String,
    pub posting_at: chrono::NaiveDateTime,
    pub lines: Vec<NewSaleLine>,
    /// Supplied PPN total (0 if tax-free; billing/tax own the computation).
    pub tax_total: Decimal,
    /// IDR receipt rounding step (e.g. 100). 0 / None = no rounding.
    pub round_to: Option<Decimal>,
}

#[derive(Debug, Clone)]
pub struct NewClose {
    pub company_id: Uuid,
    pub opening_entry_id: Uuid,
    pub cashier_party_id: Uuid,
    pub closed_at: chrono::NaiveDateTime,
    /// Counted drawer per method.
    pub counted: Vec<(String, Decimal)>,
}

#[derive(Debug, Clone)]
pub struct TenderOutcome {
    pub paid_total: Decimal,
    pub change_due: Decimal,
    pub fully_tendered: bool,
}

#[derive(Debug, Clone)]
pub struct RecognizeOutcome {
    pub pos_invoice_id: Uuid,
    pub billing_invoice_id: Uuid,
    pub payment_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ReturnOutcome {
    pub pos_invoice_id: Uuid,
    pub return_ticket_id: Uuid,
    pub billing_invoice_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct MethodRecon {
    pub method: String,
    pub expected: Decimal,
    pub counted: Decimal,
    pub difference: Decimal,
}

#[derive(Debug, Clone)]
pub struct CloseOutcome {
    pub closing_id: Uuid,
    pub difference_total: Decimal,
    pub by_method: Vec<MethodRecon>,
}

// --- errors ------------------------------------------------------------------

#[derive(Debug)]
pub enum PosError {
    EmptyDocument,
    NegativeAmount,
    SessionNotOpen,
    NotDraft,
    NotFullyTendered { rounded_total: Decimal, paid_total: Decimal },
    NotReturnable(String),
    MissingAccount(&'static str),
    DuplicateNumber(String),
    ProfileNotFound(Uuid),
    InvoiceNotFound(Uuid),
    SessionNotFound(Uuid),
    BillingRejected { code: String, message: String },
    PaymentRejected { code: String, message: String },
    Db(sqlx::Error),
}

impl PosError {
    pub fn code(&self) -> String {
        match self {
            PosError::EmptyDocument => "empty_document".into(),
            PosError::NegativeAmount => "negative_amount".into(),
            PosError::SessionNotOpen => "session_not_open".into(),
            PosError::NotDraft => "not_draft".into(),
            PosError::NotFullyTendered { .. } => "not_fully_tendered".into(),
            PosError::NotReturnable(_) => "not_returnable".into(),
            PosError::MissingAccount(_) => "missing_account".into(),
            PosError::DuplicateNumber(_) => "duplicate_number".into(),
            PosError::ProfileNotFound(_) => "profile_not_found".into(),
            PosError::InvoiceNotFound(_) => "invoice_not_found".into(),
            PosError::SessionNotFound(_) => "session_not_found".into(),
            PosError::BillingRejected { code, .. } => code.clone(),
            PosError::PaymentRejected { code, .. } => code.clone(),
            PosError::Db(_) => "internal_error".into(),
        }
    }
    pub fn http_status(&self) -> u16 {
        match self {
            PosError::ProfileNotFound(_) | PosError::InvoiceNotFound(_) | PosError::SessionNotFound(_) => 404,
            PosError::Db(_) => 500,
            _ => 422,
        }
    }
}
impl std::fmt::Display for PosError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PosError::BillingRejected { code, message } | PosError::PaymentRejected { code, message } => write!(f, "{code}: {message}"),
            other => write!(f, "{}", other.code()),
        }
    }
}
impl std::error::Error for PosError {}
impl From<sqlx::Error> for PosError {
    fn from(e: sqlx::Error) -> Self { PosError::Db(e) }
}
fn is_dup(e: &sqlx::Error) -> bool {
    e.as_database_error().map(|d| d.is_unique_violation()).unwrap_or(false)
}

#[derive(Clone)]
pub struct PosWriteService {
    db_pool: PgPool,
    sink: Arc<dyn PosEventSink>,
}

impl PosWriteService {
    pub fn new(db_pool: PgPool) -> Self {
        Self { db_pool, sink: Arc::new(LoggingSink) }
    }
    pub fn with_sink(db_pool: PgPool, sink: Arc<dyn PosEventSink>) -> Self {
        Self { db_pool, sink }
    }

    // ---- session ------------------------------------------------------------

    pub async fn open_session(&self, s: NewSession) -> Result<Uuid, PosError> {
        let id = Uuid::new_v4();
        let opening = serde_json::Value::Array(s.opening_balances.iter().map(|(m, a)| {
            serde_json::json!({ "method": m, "amount": a.to_string() })
        }).collect());
        sqlx::query(
            r#"INSERT INTO pos.pos_opening_entries
                (id, company_id, pos_profile_id, branch_id, cashier_party_id, opened_at, opening_balances, status)
               VALUES ($1,$2,$3,$4,$5,$6,$7,'open'::pos_session_status)"#,
        )
        .bind(id).bind(s.company_id).bind(s.pos_profile_id).bind(s.branch_id).bind(s.cashier_party_id)
        .bind(s.opened_at).bind(sqlx::types::Json(opening)).execute(&self.db_pool).await?;
        self.sink.publish(PosEvent::PosSessionOpened(PosSessionOpened {
            opening_entry_id: id, pos_profile_id: s.pos_profile_id, company_id: s.company_id,
        }));
        Ok(id)
    }

    // ---- ring sale ----------------------------------------------------------

    pub async fn ring_sale(&self, sale: NewSale) -> Result<Uuid, PosError> {
        if sale.lines.is_empty() { return Err(PosError::EmptyDocument); }
        // Session must be open.
        let st: Option<String> = sqlx::query_scalar("SELECT status::text FROM pos.pos_opening_entries WHERE id=$1 AND (metadata->>'deleted_at') IS NULL")
            .bind(sale.opening_entry_id).fetch_optional(&self.db_pool).await?;
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
        let tax_total = money(sale.tax_total);
        let grand = net_total + tax_total;
        let rounded = round_to(grand, sale.round_to.unwrap_or(Decimal::ZERO));
        let rounding_adjustment = rounded - grand;
        let id = Uuid::new_v4();
        let mut tx = self.db_pool.begin().await?;
        let r = sqlx::query(
            r#"INSERT INTO pos.pos_invoices
                (id, company_id, pos_profile_id, opening_entry_id, branch_id, customer_id, receipt_number,
                 posting_at, net_total, tax_total, grand_total, rounding_adjustment, rounded_total,
                 paid_total, change_due, is_return, status)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,0,0,false,'draft'::pos_invoice_status)"#,
        )
        .bind(id).bind(sale.company_id).bind(sale.pos_profile_id).bind(sale.opening_entry_id).bind(sale.branch_id)
        .bind(sale.customer_id).bind(&sale.receipt_number).bind(sale.posting_at).bind(net_total).bind(tax_total)
        .bind(grand).bind(rounding_adjustment).bind(rounded).execute(&mut *tx).await;
        if let Err(e) = r {
            return Err(if is_dup(&e) { PosError::DuplicateNumber(sale.receipt_number) } else { e.into() });
        }
        for (l, net) in &priced {
            sqlx::query(
                r#"INSERT INTO pos.pos_invoice_items
                    (id, pos_invoice_id, item_id, description, quantity, unit_price, discount_amount, net_amount, revenue_account_id)
                   VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
            )
            .bind(Uuid::new_v4()).bind(id).bind(l.item_id).bind(&l.description).bind(l.quantity)
            .bind(l.unit_price).bind(money(l.discount_amount)).bind(net).bind(l.revenue_account_id)
            .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    // ---- tender -------------------------------------------------------------

    /// Add a tender line; recompute `paid_total` + `change_due` (overpayment). Ticket must be draft.
    pub async fn add_tender(&self, pos_invoice_id: Uuid, method: &str, amount: Decimal, reference_no: Option<String>) -> Result<TenderOutcome, PosError> {
        if amount <= Decimal::ZERO { return Err(PosError::NegativeAmount); }
        let hdr = sqlx::query("SELECT status::text AS st, rounded_total FROM pos.pos_invoices WHERE id=$1 AND (metadata->>'deleted_at') IS NULL")
            .bind(pos_invoice_id).fetch_optional(&self.db_pool).await?.ok_or(PosError::InvoiceNotFound(pos_invoice_id))?;
        if hdr.get::<String, _>("st") != "draft" { return Err(PosError::NotDraft); }
        let rounded_total: Decimal = hdr.get("rounded_total");
        let mut tx = self.db_pool.begin().await?;
        sqlx::query(
            r#"INSERT INTO pos.pos_payments (id, pos_invoice_id, payment_method, amount, reference_no)
               VALUES ($1,$2,$3::pos_payment_method,$4,$5)"#,
        ).bind(Uuid::new_v4()).bind(pos_invoice_id).bind(method).bind(money(amount)).bind(&reference_no)
        .execute(&mut *tx).await?;
        let paid_total: Decimal = sqlx::query_scalar("SELECT COALESCE(SUM(amount),0) FROM pos.pos_payments WHERE pos_invoice_id=$1 AND (metadata->>'deleted_at') IS NULL")
            .bind(pos_invoice_id).fetch_one(&mut *tx).await?;
        let change_due = if paid_total > rounded_total { paid_total - rounded_total } else { Decimal::ZERO };
        sqlx::query("UPDATE pos.pos_invoices SET paid_total=$2, change_due=$3 WHERE id=$1")
            .bind(pos_invoice_id).bind(paid_total).bind(change_due).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(TenderOutcome { paid_total, change_due, fully_tendered: paid_total >= rounded_total })
    }

    // ---- recognize (the retail seam) ---------------------------------------

    /// Recognise a fully-tendered sale: drive billing (raise + post the Sales Invoice → revenue) then
    /// payment (settle the `rounded_total` against it → `Dr Cash · Cr A/R`), link the results, mark the
    /// ticket `paid`, and emit `PosInvoicePaid`. Idempotent on `status='paid'`.
    pub async fn recognize_sale(&self, pos_invoice_id: Uuid, billing: &dyn BillingPort, payment: &dyn PaymentPort) -> Result<RecognizeOutcome, PosError> {
        // Short-circuit if already recognised.
        if let Some(o) = self.short_circuit_paid(pos_invoice_id).await? { return Ok(o); }
        let inv = sqlx::query(
            r#"SELECT i.company_id, i.customer_id, i.pos_profile_id, i.posting_at, i.rounded_total, i.grand_total,
                      i.tax_total, i.paid_total, i.billing_invoice_id,
                      p.receivable_account_id, p.income_account_id, p.cash_account_id, p.currency, p.default_customer_id
               FROM pos.pos_invoices i JOIN pos.pos_profiles p ON p.id = i.pos_profile_id
               WHERE i.id=$1 AND i.status='draft'::pos_invoice_status AND (i.metadata->>'deleted_at') IS NULL"#,
        ).bind(pos_invoice_id).fetch_optional(&self.db_pool).await?.ok_or(PosError::InvoiceNotFound(pos_invoice_id))?;

        let rounded_total: Decimal = inv.get("rounded_total");
        let paid_total: Decimal = inv.get("paid_total");
        if paid_total < rounded_total {
            return Err(PosError::NotFullyTendered { rounded_total, paid_total });
        }
        let tax_total: Decimal = inv.get("tax_total");
        if tax_total > Decimal::ZERO { return Err(PosError::MissingAccount("tax account (PPN not yet configured on the profile)")); }
        let company_id: Uuid = inv.get("company_id");
        let currency: String = inv.get("currency");
        let receivable: Uuid = inv.get::<Option<Uuid>, _>("receivable_account_id").ok_or(PosError::MissingAccount("receivable_account_id"))?;
        let income: Option<Uuid> = inv.get("income_account_id");
        let cash: Uuid = inv.get::<Option<Uuid>, _>("cash_account_id").ok_or(PosError::MissingAccount("cash_account_id"))?;
        let customer: Uuid = inv.get::<Option<Uuid>, _>("customer_id")
            .or_else(|| inv.get::<Option<Uuid>, _>("default_customer_id"))
            .ok_or(PosError::MissingAccount("customer_id (no ticket or profile default)"))?;

        let posting_at: chrono::DateTime<chrono::Utc> = inv.get("posting_at");
        let posting_date = posting_at.date_naive();

        // Billing at-most-once (council 2026-07-05): raise the Sales Invoice only if it was not already
        // raised on a prior attempt. Persisting `billing_invoice_id` on the still-draft ticket the
        // instant `raise_and_post` returns makes a retry after a settle failure (declined card / closed
        // period) REUSE the invoice instead of raising a second one — otherwise the counter's ordinary
        // decline-then-retry doubles the revenue journal (billing does not dedup on the POS ticket).
        let existing_billing: Option<Uuid> = inv.get("billing_invoice_id");
        let invoice_id = if let Some(id) = existing_billing {
            id
        } else {
            // Sale lines → invoice request (revenue = line account, else profile income).
            let item_rows = sqlx::query("SELECT item_id, net_amount, revenue_account_id FROM pos.pos_invoice_items WHERE pos_invoice_id=$1 AND (metadata->>'deleted_at') IS NULL")
                .bind(pos_invoice_id).fetch_all(&self.db_pool).await?;
            let mut lines = Vec::with_capacity(item_rows.len());
            for r in &item_rows {
                let rev = r.get::<Option<Uuid>, _>("revenue_account_id").or(income).ok_or(PosError::MissingAccount("revenue account (line or profile income)"))?;
                // Pass the NET as the unit_price at qty 1 so billing's line net == POS net (discounts already applied).
                lines.push(SaleLine { item_id: r.get("item_id"), revenue_account_id: rev, quantity: Decimal::ONE, unit_price: r.get("net_amount") });
            }
            let ack = billing.raise_and_post(&SaleInvoiceRequest {
                company_id, customer_id: customer, currency: currency.clone(), source_pos_id: pos_invoice_id,
                receivable_account_id: receivable, posting_date, lines, tax_total: Decimal::ZERO, tax_account_id: None,
            }).await.map_err(|e| PosError::BillingRejected { code: e.code, message: e.message })?;
            // Persist the link WHILE STILL DRAFT — before settle — so any retry reuses this invoice.
            sqlx::query("UPDATE pos.pos_invoices SET billing_invoice_id=$2 WHERE id=$1 AND status='draft'::pos_invoice_status")
                .bind(pos_invoice_id).bind(ack.invoice_id).execute(&self.db_pool).await?;
            ack.invoice_id
        };

        // Settle the rounded_total against the raised invoice (cash sale → A/R nets to zero).
        let sack = payment.settle(&SettlementRequest {
            company_id, customer_id: customer, currency, invoice_ref: invoice_id,
            bank_account_id: cash, party_account_id: receivable, posting_date, amount: rounded_total,
        }).await.map_err(|e| PosError::PaymentRejected { code: e.code, message: e.message })?;

        // Gate the event on THIS invocation performing the draft→paid transition.
        let res = sqlx::query(
            "UPDATE pos.pos_invoices SET status='paid'::pos_invoice_status, billing_invoice_id=$2 WHERE id=$1 AND status='draft'::pos_invoice_status",
        ).bind(pos_invoice_id).bind(invoice_id).execute(&self.db_pool).await?;
        if res.rows_affected() == 1 {
            self.sink.publish(PosEvent::PosInvoicePaid(PosInvoicePaid {
                pos_invoice_id, company_id, grand_total: inv.get("grand_total"), rounded_total,
                billing_invoice_id: invoice_id, payment_id: sack.payment_id,
            }));
        }
        Ok(RecognizeOutcome { pos_invoice_id, billing_invoice_id: invoice_id, payment_id: sack.payment_id })
    }

    async fn short_circuit_paid(&self, pos_invoice_id: Uuid) -> Result<Option<RecognizeOutcome>, PosError> {
        let row = sqlx::query("SELECT status::text AS st, billing_invoice_id FROM pos.pos_invoices WHERE id=$1 AND (metadata->>'deleted_at') IS NULL")
            .bind(pos_invoice_id).fetch_optional(&self.db_pool).await?.ok_or(PosError::InvoiceNotFound(pos_invoice_id))?;
        if row.get::<String, _>("st") == "paid" {
            if let Some(bid) = row.get::<Option<Uuid>, _>("billing_invoice_id") {
                return Ok(Some(RecognizeOutcome { pos_invoice_id, billing_invoice_id: bid, payment_id: Uuid::nil() }));
            }
        }
        Ok(None)
    }

    // ---- return (the retail returns path) ----------------------------------

    /// Return a recognised sale (council 2026-07-05, the KEEP returns flow). Drives billing (credit
    /// note — `Dr Revenue · Cr A/R`, invoice → cancelled) and payment (refund — `Dr A/R · Cr Cash`),
    /// so revenue, cash, and A/R all net back to zero. Records an `is_return` ticket linked via
    /// `return_against`, flips the original → `returned`, emits `PosInvoiceReturned`. **Idempotent:**
    /// both downstream reversals are idempotent, and the ticket/event are gated on the original's
    /// `paid→returned` transition — a repeat return refunds/credits at most once. POS posts no GL.
    pub async fn return_sale(&self, original_pos_invoice_id: Uuid, billing: &dyn BillingPort, payment: &dyn PaymentPort) -> Result<ReturnOutcome, PosError> {
        let o = sqlx::query(
            r#"SELECT company_id, pos_profile_id, opening_entry_id, branch_id, customer_id, billing_invoice_id,
                      rounded_total, net_total, tax_total, grand_total, posting_at, status::text AS st
               FROM pos.pos_invoices WHERE id=$1 AND (metadata->>'deleted_at') IS NULL"#,
        ).bind(original_pos_invoice_id).fetch_optional(&self.db_pool).await?.ok_or(PosError::InvoiceNotFound(original_pos_invoice_id))?;
        let status: String = o.get("st");
        if status != "paid" && status != "returned" {
            return Err(PosError::NotReturnable(status));
        }
        let billing_invoice_id: Uuid = o.get::<Option<Uuid>, _>("billing_invoice_id").ok_or(PosError::NotReturnable("no billing invoice".into()))?;
        let company_id: Uuid = o.get("company_id");
        let rounded_total: Decimal = o.get("rounded_total");

        // Drive the two reversals (both idempotent downstream): refund the tender + credit-note the sale.
        payment.refund(&RefundRequest { company_id, invoice_ref: billing_invoice_id, amount: rounded_total })
            .await.map_err(|e| PosError::PaymentRejected { code: e.code, message: e.message })?;
        billing.credit_note(&CreditNoteRequest { company_id, invoice_ref: billing_invoice_id })
            .await.map_err(|e| PosError::BillingRejected { code: e.code, message: e.message })?;

        // Gate the return ticket + event on the original's paid→returned transition (exactly-once).
        let res = sqlx::query("UPDATE pos.pos_invoices SET status='returned'::pos_invoice_status WHERE id=$1 AND status='paid'::pos_invoice_status")
            .bind(original_pos_invoice_id).execute(&self.db_pool).await?;
        let return_ticket_id = if res.rows_affected() == 1 {
            let rt = Uuid::new_v4();
            sqlx::query(
                r#"INSERT INTO pos.pos_invoices
                    (id, company_id, pos_profile_id, opening_entry_id, branch_id, customer_id, receipt_number,
                     posting_at, net_total, tax_total, grand_total, rounding_adjustment, rounded_total,
                     paid_total, change_due, billing_invoice_id, is_return, return_against, status)
                   VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,0,$12,$12,0,$13,true,$14,'returned'::pos_invoice_status)"#,
            )
            .bind(rt).bind(company_id).bind(o.get::<Uuid, _>("pos_profile_id")).bind(o.get::<Uuid, _>("opening_entry_id"))
            .bind(o.get::<Option<Uuid>, _>("branch_id")).bind(o.get::<Option<Uuid>, _>("customer_id"))
            .bind(format!("RET-{}", &original_pos_invoice_id.simple().to_string()[..12]))
            .bind(o.get::<chrono::DateTime<chrono::Utc>, _>("posting_at")).bind(o.get::<Decimal, _>("net_total"))
            .bind(o.get::<Decimal, _>("tax_total")).bind(o.get::<Decimal, _>("grand_total")).bind(rounded_total)
            .bind(billing_invoice_id).bind(original_pos_invoice_id).execute(&self.db_pool).await?;
            self.sink.publish(PosEvent::PosInvoiceReturned(PosInvoiceReturned {
                pos_invoice_id: original_pos_invoice_id, return_ticket_id: rt, company_id,
                billing_invoice_id, amount: rounded_total,
            }));
            rt
        } else {
            // Already returned — reuse the existing return ticket (idempotent replay).
            sqlx::query_scalar("SELECT id FROM pos.pos_invoices WHERE return_against=$1 AND is_return=true AND (metadata->>'deleted_at') IS NULL LIMIT 1")
                .bind(original_pos_invoice_id).fetch_optional(&self.db_pool).await?.unwrap_or(original_pos_invoice_id)
        };
        Ok(ReturnOutcome { pos_invoice_id: original_pos_invoice_id, return_ticket_id, billing_invoice_id })
    }

    // ---- close (drawer reconciliation) -------------------------------------

    /// Close a session: for each tender method, `expected = opening_float + Σ recognised tenders`
    /// (cash also `− Σ change_due`); `difference = counted − expected`. Persist the per-method
    /// breakdown + the session's grand total, mark the session closed, emit `PosSessionClosed`.
    pub async fn close_session(&self, c: NewClose) -> Result<CloseOutcome, PosError> {
        let opening_json: Option<sqlx::types::Json<serde_json::Value>> = sqlx::query_scalar(
            "SELECT opening_balances FROM pos.pos_opening_entries WHERE id=$1 AND company_id=$2 AND (metadata->>'deleted_at') IS NULL",
        ).bind(c.opening_entry_id).bind(c.company_id).fetch_optional(&self.db_pool).await?
            .ok_or(PosError::SessionNotFound(c.opening_entry_id))?;
        let mut expected: BTreeMap<String, Decimal> = BTreeMap::new();
        if let Some(sqlx::types::Json(serde_json::Value::Array(arr))) = opening_json {
            for e in arr {
                if let (Some(m), Some(a)) = (e.get("method").and_then(|v| v.as_str()), e.get("amount").and_then(|v| v.as_str())) {
                    *expected.entry(m.to_string()).or_insert(Decimal::ZERO) += Decimal::from_str_exact(a).unwrap_or(Decimal::ZERO);
                }
            }
        }
        // Σ tenders per method for recognised (paid) tickets in the session.
        let tender_rows = sqlx::query(
            r#"SELECT pay.payment_method::text AS method, COALESCE(SUM(pay.amount),0) AS total
               FROM pos.pos_payments pay JOIN pos.pos_invoices i ON i.id = pay.pos_invoice_id
               WHERE i.opening_entry_id=$1 AND i.status='paid'::pos_invoice_status AND (pay.metadata->>'deleted_at') IS NULL
               GROUP BY pay.payment_method"#,
        ).bind(c.opening_entry_id).fetch_all(&self.db_pool).await?;
        for r in &tender_rows {
            *expected.entry(r.get::<String, _>("method")).or_insert(Decimal::ZERO) += r.get::<Decimal, _>("total");
        }
        // Cash change reduces the drawer.
        let cash_change: Decimal = sqlx::query_scalar(
            "SELECT COALESCE(SUM(change_due),0) FROM pos.pos_invoices WHERE opening_entry_id=$1 AND status='paid'::pos_invoice_status AND (metadata->>'deleted_at') IS NULL",
        ).bind(c.opening_entry_id).fetch_one(&self.db_pool).await?;
        if cash_change > Decimal::ZERO {
            *expected.entry("cash".to_string()).or_insert(Decimal::ZERO) -= cash_change;
        }
        // Session totals.
        let (grand_total, invoice_count): (Decimal, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(rounded_total),0), COUNT(*) FROM pos.pos_invoices WHERE opening_entry_id=$1 AND status='paid'::pos_invoice_status AND (metadata->>'deleted_at') IS NULL",
        ).bind(c.opening_entry_id).fetch_one(&self.db_pool).await?;

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
        let profile_id: Uuid = sqlx::query_scalar("SELECT pos_profile_id FROM pos.pos_opening_entries WHERE id=$1").bind(c.opening_entry_id).fetch_one(&mut *tx).await?;
        sqlx::query(
            r#"INSERT INTO pos.pos_closing_entries
                (id, company_id, pos_profile_id, opening_entry_id, closed_at, cashier_party_id,
                 totals_by_method, grand_total, invoice_count, difference_total, status)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'submitted'::pos_closing_status)"#,
        )
        .bind(id).bind(c.company_id).bind(profile_id).bind(c.opening_entry_id).bind(c.closed_at).bind(c.cashier_party_id)
        .bind(sqlx::types::Json(totals_json)).bind(money(grand_total)).bind(invoice_count as i32).bind(money(difference_total))
        .execute(&mut *tx).await?;
        sqlx::query("UPDATE pos.pos_opening_entries SET status='closed'::pos_session_status WHERE id=$1")
            .bind(c.opening_entry_id).execute(&mut *tx).await?;
        tx.commit().await?;

        self.sink.publish(PosEvent::PosSessionClosed(PosSessionClosed {
            closing_entry_id: id, opening_entry_id: c.opening_entry_id, company_id: c.company_id,
            difference_total: money(difference_total),
        }));
        Ok(CloseOutcome { closing_id: id, difference_total: money(difference_total), by_method })
    }
}
