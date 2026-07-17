//! Validated write path + retail orchestrator for POS (hand-authored, user-owned).
//!
//! POS owns the cashier session + the counter ticket, but posts NO GL: on `recognize_sale` it drives
//! `backbone-billing` (raise + post the real Sales Invoice — revenue) and `backbone-payment` (settle
//! the tender) through the `BillingPort`/`PaymentPort`, so the retail path reuses the same GL emitters
//! as web/B2B sales. A cash sale therefore books `Dr A/R · Cr Revenue` (billing) then `Dr Cash · Cr
//! A/R` (payment) — A/R nets to zero at the counter. Close reconciles the drawer per tender method.
//!
//! **Layering (the module's 4-layer rule):** this service ORCHESTRATES — it validates, computes the
//! money, owns the unit of work (`begin`/`commit`), drives the ports, and publishes events. It holds
//! no SQL: every statement lives on the repositories in `infrastructure::persistence`, whose custom
//! methods take the caller's transaction so a cross-entity write (ticket header + its lines) commits
//! as one unit. The RLS scope wrappers (ADR-0008) stay HERE, in the service, because the service is
//! what knows the company; tx-taking repo methods ride the bind this service already made.
//!
//! **This file is the hub:** it holds the module's vocabulary (input structs, outcomes, errors) and
//! the session-open path. The rest of the write surface is chunked into focused siblings, each an
//! `impl PosWriteService` block over these same types:
//!
//! - [`super::pos_sale`] — ring a ticket (`ring_sale`, `ring_sale_priced`).
//! - [`super::pos_tender`] — take payment at the counter (`add_tender`).
//! - [`super::pos_recognition`] — the retail seam: `recognize_sale` / `return_sale`.
//! - [`super::pos_drawer`] — cash movements, the X/Z drawer read, `close_session`.
//! - [`super::pos_receipt`] — assemble the printed slip.

use backbone_orm::company_scope;
use rust_decimal::{Decimal, RoundingStrategy};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use crate::infrastructure::persistence::{
    NewOpeningEntryRow, PosCashMovementRepository, PosClosingEntryRepository, PosInvoiceItemRepository,
    PosInvoiceRepository, PosOpeningEntryRepository, PosPaymentRepository, PosProfileRepository,
};

use super::pos_events::{PosEvent, PosEventSink, PosSessionOpened, LoggingSink};

pub(super) fn money(v: Decimal) -> Decimal {
    v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}
/// Round `v` to the nearest `step` (IDR receipt rounding; `step == 0` means no rounding).
pub(super) fn round_to(v: Decimal, step: Decimal) -> Decimal {
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

/// One ticket line to be priced by the promo cart pricer — list price + the dimensions promo matches
/// bundles/rules on (item group, brand), which a plain `NewSaleLine` does not carry.
#[derive(Debug, Clone)]
pub struct CartSaleLine {
    pub item_id: Uuid,
    pub item_group_id: Option<Uuid>,
    pub brand_id: Option<Uuid>,
    pub revenue_account_id: Option<Uuid>,
    pub description: Option<String>,
    pub list_price: Decimal,
    pub quantity: Decimal,
}

/// A ticket rung through the promo cart seam (`ring_sale_priced`).
#[derive(Debug, Clone)]
pub struct NewCartSale {
    pub company_id: Uuid,
    pub pos_profile_id: Uuid,
    pub opening_entry_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub customer_group_id: Option<Uuid>,
    pub coupon_code: Option<String>,
    pub receipt_number: String,
    pub posting_at: chrono::NaiveDateTime,
    pub tax_total: Decimal,
    pub round_to: Option<Decimal>,
    pub lines: Vec<CartSaleLine>,
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
pub struct NewCashMovement {
    pub company_id: Uuid,
    pub pos_profile_id: Uuid,
    pub opening_entry_id: Uuid,
    pub cashier_party_id: Uuid,
    /// One of: `pay_in`, `pay_out`, `drop`, `no_sale`.
    pub movement_type: String,
    /// Positive cash amount; must be 0 for `no_sale`.
    pub amount: Decimal,
    pub reason: Option<String>,
    pub moved_at: chrono::NaiveDateTime,
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

#[derive(Debug, Clone)]
pub struct MethodExpected {
    pub method: String,
    pub expected: Decimal,
}

/// A mid-shift drawer read (X-report): expected drawer per method + running session totals, with no
/// counting and no close.
#[derive(Debug, Clone)]
pub struct XReport {
    pub opening_entry_id: Uuid,
    pub by_method: Vec<MethodExpected>,
    pub grand_total: Decimal,
    pub invoice_count: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReceiptLine {
    pub description: String,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    pub discount_amount: Decimal,
    pub net_amount: Decimal,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReceiptTender {
    pub method: String,
    pub amount: Decimal,
}

/// A rendered sale receipt: the line items + money breakdown (incl. PPN) + tender/change a printer or
/// display needs. `render_text()` produces a monospace slip.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub pos_invoice_id: Uuid,
    pub receipt_number: String,
    pub register_name: String,
    pub posting_at: chrono::DateTime<chrono::Utc>,
    pub currency: String,
    pub status: String,
    pub lines: Vec<ReceiptLine>,
    pub net_total: Decimal,
    pub tax_rate: Decimal,
    pub tax_total: Decimal,
    pub rounding_adjustment: Decimal,
    pub grand_total: Decimal,
    pub tenders: Vec<ReceiptTender>,
    pub paid_total: Decimal,
    pub change_due: Decimal,
}

impl Receipt {
    /// Render a 32-column monospace slip: header, line items, subtotal, PPN (if any), rounding (if any),
    /// total, tenders, change. Deliberately printer-agnostic text — an ESC/POS driver wraps it.
    pub fn render_text(&self) -> String {
        const W: usize = 32;
        let money = |d: Decimal| format!("{:.2}", d);
        let row = |left: &str, right: &str| -> String {
            let r = right.to_string();
            let l_max = W.saturating_sub(r.len() + 1);
            let l: String = left.chars().take(l_max).collect();
            format!("{l}{}{r}", " ".repeat(W.saturating_sub(l.chars().count() + r.len())))
        };
        let center = |s: &str| -> String {
            let pad = W.saturating_sub(s.chars().count()) / 2;
            format!("{}{s}", " ".repeat(pad))
        };
        let rule = "-".repeat(W);
        let mut o = String::new();
        o.push_str(&center(&self.register_name)); o.push('\n');
        o.push_str(&center(&format!("Receipt {}", self.receipt_number))); o.push('\n');
        o.push_str(&center(&self.posting_at.format("%Y-%m-%d %H:%M").to_string())); o.push('\n');
        o.push_str(&rule); o.push('\n');
        for l in &self.lines {
            o.push_str(&l.description); o.push('\n');
            let qty_price = format!("  {} x {}", l.quantity.normalize(), money(l.unit_price));
            o.push_str(&row(&qty_price, &money(l.net_amount))); o.push('\n');
            if l.discount_amount > Decimal::ZERO {
                o.push_str(&row("  disc", &format!("-{}", money(l.discount_amount)))); o.push('\n');
            }
        }
        o.push_str(&rule); o.push('\n');
        o.push_str(&row("Subtotal", &money(self.net_total))); o.push('\n');
        if self.tax_total > Decimal::ZERO {
            let pct = (self.tax_rate * Decimal::from(100)).normalize();
            o.push_str(&row(&format!("PPN {pct}%"), &money(self.tax_total))); o.push('\n');
        }
        if self.rounding_adjustment != Decimal::ZERO {
            o.push_str(&row("Rounding", &money(self.rounding_adjustment))); o.push('\n');
        }
        o.push_str(&row("TOTAL", &money(self.grand_total))); o.push('\n');
        o.push_str(&rule); o.push('\n');
        for t in &self.tenders {
            o.push_str(&row(&t.method, &money(t.amount))); o.push('\n');
        }
        if self.change_due > Decimal::ZERO {
            o.push_str(&row("Change", &money(self.change_due))); o.push('\n');
        }
        o.push_str(&rule); o.push('\n');
        o.push_str(&center("Terima kasih")); o.push('\n');
        o
    }
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
    PricingRejected { code: String, message: String },
    InventoryRejected { code: String, message: String },
    InvalidCashMovement(&'static str),
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
            PosError::PricingRejected { code, .. } => code.clone(),
            PosError::InventoryRejected { code, .. } => code.clone(),
            PosError::InvalidCashMovement(_) => "invalid_cash_movement".into(),
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

/// Discriminate a unique violation out of a raw `sqlx::Error`.
///
/// This is why the repositories' write methods leak `sqlx::Error` rather than a typed repo error: the
/// service turns a re-used receipt number into `DuplicateNumber`, and a typed error would have thrown
/// that information away.
pub(super) fn is_dup(e: &sqlx::Error) -> bool {
    e.as_database_error().map(|d| d.is_unique_violation()).unwrap_or(false)
}

/// The repositories are held behind `Arc` only so this service stays `Clone` (its HTTP surface clones
/// it per request) — `GenericCrudRepository` is not itself `Clone`. They are stateless handles over the
/// same pool; the `Arc` carries no shared mutable state.
#[derive(Clone)]
pub struct PosWriteService {
    pub(super) db_pool: PgPool,
    pub(super) sink: Arc<dyn PosEventSink>,
    pub(super) invoices: Arc<PosInvoiceRepository>,
    pub(super) items: Arc<PosInvoiceItemRepository>,
    pub(super) payments: Arc<PosPaymentRepository>,
    pub(super) profiles: Arc<PosProfileRepository>,
    pub(super) openings: Arc<PosOpeningEntryRepository>,
    pub(super) closings: Arc<PosClosingEntryRepository>,
    pub(super) movements: Arc<PosCashMovementRepository>,
}

impl PosWriteService {
    pub fn new(db_pool: PgPool) -> Self {
        Self::with_sink(db_pool, Arc::new(LoggingSink))
    }
    pub fn with_sink(db_pool: PgPool, sink: Arc<dyn PosEventSink>) -> Self {
        Self {
            invoices: Arc::new(PosInvoiceRepository::new(db_pool.clone())),
            items: Arc::new(PosInvoiceItemRepository::new(db_pool.clone())),
            payments: Arc::new(PosPaymentRepository::new(db_pool.clone())),
            profiles: Arc::new(PosProfileRepository::new(db_pool.clone())),
            openings: Arc::new(PosOpeningEntryRepository::new(db_pool.clone())),
            closings: Arc::new(PosClosingEntryRepository::new(db_pool.clone())),
            movements: Arc::new(PosCashMovementRepository::new(db_pool.clone())),
            db_pool,
            sink,
        }
    }

    // ---- session ------------------------------------------------------------

    pub async fn open_session(&self, s: NewSession) -> Result<Uuid, PosError> {
        // RLS scope (ADR-0008): bind this call to its own company for the whole body, so every query
        // runs with `app.company_id` set — via the request-dedicated connection under HTTP, or a
        // per-statement scope for non-request callers (jobs). The explicit `company_id` binds below
        // stay as defense-in-depth. This is the pattern every custom write service should follow.
        let company = s.company_id;
        company_scope::with_company_scope(Some(company), async move {
            let id = Uuid::new_v4();
            let opening = serde_json::Value::Array(s.opening_balances.iter().map(|(m, a)| {
                serde_json::json!({ "method": m, "amount": a.to_string() })
            }).collect());
            self.openings.insert_opening_entry(&self.db_pool, &NewOpeningEntryRow {
                id,
                company_id: s.company_id,
                pos_profile_id: s.pos_profile_id,
                branch_id: s.branch_id,
                cashier_party_id: s.cashier_party_id,
                opened_at: s.opened_at,
                opening_balances: opening,
            }).await?;
            self.sink.publish(PosEvent::PosSessionOpened(PosSessionOpened {
                opening_entry_id: id, pos_profile_id: s.pos_profile_id, company_id: s.company_id,
            }));
            Ok(id)
        }).await
    }
}
