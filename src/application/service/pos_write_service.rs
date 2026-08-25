//! Validated write path + retail orchestrator for POS (hand-authored, user-owned).
//!
//! POS owns the cashier session + the counter ticket, but posts NO GL: on `recognize_sale` it drives
//! `backbone-billing` (raise + post the real Sales Invoice — revenue) and `backbone-payment` (settle
//! the tender) through the `BillingPort`/`PaymentPort`, so the retail path reuses the same GL emitters
//! as web/B2B sales. A cash sale therefore books `Dr A/R · Cr Revenue` (billing) then `Dr Cash · Cr
//! A/R` (payment) — A/R nets to zero at the counter. Close reconciles the drawer per tender method
//! and books any variance through the `PosCashVariancePort`.
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
//! - [`super::pos_compute`] — the server-owned ticket computation (document-grade tax + register
//!   cash rounding) shared by every entry point that rings a ticket.
//! - [`super::pos_sale`] — ring a ticket (`ring_sale`, `ring_sale_priced`).
//! - [`super::pos_tender`] — take payment at the counter (`add_tender`).
//! - [`super::pos_recognition`] — the retail seam: `recognize_sale` / `return_sale`.
//! - [`super::pos_drawer`] — cash movements, the X/Z drawer read, `close_session`.
//! - [`super::pos_receipt`] — assemble the printed slip.
//! - [`super::pos_sync`] — the offline reconciliation verb (`sync_from_ui`).
//! - [`super::pos_manager_pin`] — the manager-PIN credential path (`set_pin`, `verify_pin`).
//! - [`super::pos_discount`] — the order-level discount resolution + fold (percentage from the
//!   company's discount master, never a client-echoed rate).
//! - [`super::pos_session_alert`] — the old-session alert scheduler handler (pickup-locked).

use backbone_orm::company_scope;
use rust_decimal::{Decimal, RoundingStrategy};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::infrastructure::persistence::{
    NewOpeningEntryRow, PosCashMovementRepository, PosClosingEntryRepository, PosDiscountRepository,
    PosInvoiceItemRepository, PosInvoiceRepository, PosManagerPinRepository, PosOpeningEntryRepository,
    PosPaymentRepository, PosProfileRepository, PosTableRepository,
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
    /// Course grouping for kitchen routing (restaurant): lines sharing a course number fire
    /// together. `None` = not course-grouped (a plain counter line).
    pub course: Option<i32>,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    pub discount_amount: Decimal,
}

/// A ticket rung online. Totals are NEVER taken from here: tax is computed document-grade through
/// the register's templates (`PosTaxComputePort`) and cash rounding comes from the register's
/// configuration — the shared compute core in [`super::pos_compute`] owns every money field written.
/// `pos_table_id` seats the ticket at a dining table (one live draft per table is enforced — the
/// restaurant invariant); `discount_id` names an order-level discount MASTER row whose stored
/// percentage the server applies (a client-echoed rate is never read).
#[derive(Debug, Clone)]
pub struct NewSale {
    pub company_id: Uuid,
    pub pos_profile_id: Uuid,
    pub opening_entry_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub pos_table_id: Option<Uuid>,
    pub discount_id: Option<Uuid>,
    pub receipt_number: String,
    pub posting_at: chrono::NaiveDateTime,
    pub lines: Vec<NewSaleLine>,
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
    /// Course grouping for kitchen routing (restaurant), carried through the pricing round-trip.
    pub course: Option<i32>,
    pub list_price: Decimal,
    pub quantity: Decimal,
}

/// A ticket rung through the promo cart seam (`ring_sale_priced`). Totals are server-owned exactly
/// as on [`NewSale`]; `pos_table_id` / `discount_id` carry the same meaning.
#[derive(Debug, Clone)]
pub struct NewCartSale {
    pub company_id: Uuid,
    pub pos_profile_id: Uuid,
    pub opening_entry_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub customer_group_id: Option<Uuid>,
    pub coupon_code: Option<String>,
    pub pos_table_id: Option<Uuid>,
    pub discount_id: Option<Uuid>,
    pub receipt_number: String,
    pub posting_at: chrono::NaiveDateTime,
    pub lines: Vec<CartSaleLine>,
}

/// A privileged mutation's proof of manager identity: the employee whose PIN it is + the PIN itself.
/// The PIN is verified SERVER-SIDE against the stored argon2 hash on every privileged verb — a
/// client-side "manager mode" flag is a UI affordance and never authorizes anything here.
#[derive(Debug, Clone)]
pub struct ManagerAuth {
    pub employee_party_id: Uuid,
    pub pin: String,
}

#[derive(Debug, Clone)]
pub struct NewClose {
    pub company_id: Uuid,
    pub opening_entry_id: Uuid,
    pub cashier_party_id: Uuid,
    pub closed_at: chrono::NaiveDateTime,
    /// Counted drawer per method (the cash statement legs).
    pub counted: Vec<(String, Decimal)>,
    /// Closing the till is a privileged mutation: the manager's PIN is verified server-side before
    /// anything is written (variance is booked to the GL through the register's write-off account).
    pub manager: ManagerAuth,
    /// Best-effort source address feeding the PIN throttle ring (`None` = service-to-service caller).
    pub source_ip: Option<String>,
}

// --- offline sync (sync_from_ui) ----------------------------------------------

/// One ticket line as an offline client replays it. `client_uuid` is the line's sync identity
/// (DISTINCT from the server-side row id); `unit_price`/`quantity`/`discount_amount` are inputs to
/// pricing resolution exactly as `ring_sale` treats them — the totals are always server-derived.
#[derive(Debug, Clone)]
pub struct SyncSaleLine {
    pub client_uuid: Uuid,
    pub item_id: Uuid,
    pub revenue_account_id: Option<Uuid>,
    pub description: Option<String>,
    /// Course grouping for kitchen routing (restaurant), replayed with the basket.
    pub course: Option<i32>,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    pub discount_amount: Decimal,
}

/// One tender as an offline client replays it: the method + the amount actually taken at the
/// counter (the client is the record of what changed hands; the SERVER re-sums what these imply).
#[derive(Debug, Clone)]
pub struct SyncTender {
    pub client_uuid: Uuid,
    pub method: String,
    pub amount: Decimal,
    pub reference_no: Option<String>,
}

/// An offline ticket replayed at the server. Client totals (`amount_paid` / `amount_total` /
/// `amount_tax` / `amount_return`) are DISCARDED by design — the server recomputes every money
/// field from the lines + tenders through the same compute core `ring_sale` uses. Identity is the
/// ticket's `client_uuid`: a replay whose uuid already exists UPDATES that ticket (when still
/// mutable) instead of creating a second one.
#[derive(Debug, Clone)]
pub struct NewSyncSale {
    pub company_id: Uuid,
    pub client_uuid: Uuid,
    pub pos_profile_id: Uuid,
    /// The session the client rang the ticket under. Validated, not trusted: if that session has
    /// since CLOSED, the ticket is refused unless `rescue_opening_entry_id` names a session that is
    /// currently open (the rescue).
    pub opening_entry_id: Uuid,
    /// Optional open session to attach the ticket to when the original session has closed.
    pub rescue_opening_entry_id: Option<Uuid>,
    pub branch_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    /// The dining table the replay seats the ticket at. On CREATE it is the seating; on UPDATE a
    /// changed value is a table TRANSFER — validated against the one-draft-per-table guard.
    pub pos_table_id: Option<Uuid>,
    /// Names the order-level discount MASTER row; the server reads the percentage from the master
    /// (never the replay's echo of one).
    pub discount_id: Option<Uuid>,
    pub posting_at: chrono::NaiveDateTime,
    pub lines: Vec<SyncSaleLine>,
    pub tenders: Vec<SyncTender>,
    /// For a refund replay: the PARENT ticket's client uuid. At most one parent per refund ticket
    /// (enforced structurally — a ticket referencing more than one distinct refunded parent is
    /// refused). `Some(_)` makes the sync a privileged mutation: `manager` is then required.
    pub refund_of_client_uuid: Option<Uuid>,
    /// Manager authorization — required iff `refund_of_client_uuid` is set.
    pub manager: Option<ManagerAuth>,
    /// Where the replay claims to come from — feeds the PIN throttle ring on privileged replays.
    /// Optional: a service-to-service caller (no address) skips the per-address budget.
    pub source_ip: Option<String>,
}

/// How a `sync_from_ui` call landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    /// No live ticket carried the uuid — a new one was created.
    Created,
    /// A live DRAFT ticket carried the uuid — its lines/tenders/totals were rewritten.
    Updated,
    /// The ticket carrying the uuid is already finalized server-side (paid/returned) — the replay
    /// changed nothing; the server's state is authoritative.
    ReplayFinalized,
}

#[derive(Debug, Clone)]
pub struct SyncOutcome {
    pub pos_invoice_id: Uuid,
    pub action: SyncAction,
    pub totals: TicketTotals,
}

/// The server-derived money of a ticket — every field computed by [`super::pos_compute`], none of
/// them ever read from a client payload.
#[derive(Debug, Clone, PartialEq)]
pub struct TicketTotals {
    pub net_total: Decimal,
    pub tax_total: Decimal,
    pub grand_total: Decimal,
    pub rounding_adjustment: Decimal,
    pub rounded_total: Decimal,
    pub paid_total: Decimal,
    pub change_due: Decimal,
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

/// The booked drawer variance of a close: which journal absorbed it and in which direction.
#[derive(Debug, Clone)]
pub struct VarianceBooking {
    pub journal_id: Uuid,
    pub amount: Decimal,
    pub direction: super::pos_ports::CashVarianceDirection,
}

#[derive(Debug, Clone)]
pub struct CloseOutcome {
    pub closing_id: Uuid,
    pub difference_total: Decimal,
    pub by_method: Vec<MethodRecon>,
    /// The GL correction booked for a non-zero drawer difference (`None` when the drawer balanced
    /// exactly — no journal is raised for a zero variance).
    pub variance: Option<VarianceBooking>,
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
    /// Course grouping for kitchen routing (restaurant) — surfaced on the read side so kitchen
    /// displays and receipt renderers can group firing lines; `None` on counter lines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub course: Option<i32>,
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
    /// Whether this ticket has been recognized into a billing invoice — DERIVED from the ticket's
    /// billing link (`billing_invoice_id IS NOT NULL`), never a stored state column. A receipt is
    /// "invoiced" the moment recognition attached the real Sales Invoice.
    pub is_invoiced: bool,
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
    /// The open session belongs to a different register than the ticket names — a ticket must
    /// settle on its own register's drawer; pairing one register's config with another's till
    /// would misattribute cash between registers.
    SessionRegisterMismatch,
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
    /// Forward-compatible seam (council 2026-07-26, #5): `return_sale` accepts an optional
    /// `PartialCredit`, but partial-return behavior is not yet implemented — it lands when
    /// backbone-billing's credit note honors a line subset (ADR-001 park; gate: merchant demand).
    PartialReturnsNotImplemented,

    // --- document-grade tax + register configuration -------------------------
    /// The register carries no tax templates — a ticket cannot be priced. Configure at least one
    /// template on the profile (zero-rated included for a non-PKP register).
    ProfileTaxTemplatesMissing(Uuid),
    /// The tax compute port refused the request (propagates the seam's code + message).
    TaxRejected { code: String, message: String },
    /// The session the client rang under has since closed and no open rescue session was offered.
    SessionClosedRescueRequired(Uuid),

    // --- offline sync ---------------------------------------------------------
    /// The replay would reassign the ticket to a different customer than it was first synced with.
    SyncPartnerMismatch,
    /// The replay names a session the ticket does not belong to (and the original is still open).
    SyncSessionMismatch,
    /// A refund replay's parent uuid does not resolve to a live ticket of this company.
    RefundParentNotFound(Uuid),
    /// A refund ticket already recorded against a different parent refuses to be re-pointed.
    RefundLineageConflict,
    /// A tender replay carries a method the payment-method enum does not know.
    InvalidTenderMethod(String),
    /// The client uuid is already held by another live ticket of this company (the sync identity
    /// namespaces inside the tenant; a collision is a client bug or a replay of someone else's uuid).
    DuplicateClientUuid(Uuid),

    // --- session close guards --------------------------------------------------
    /// The session still holds DRAFT tickets — post or void them before closing.
    SessionHasDraftOrders(i64),
    /// The session holds invoiced tickets whose posting did not complete (billing linked, ticket
    /// not paid) — retry recognition before closing.
    SessionHasUnpostedInvoices(i64),
    /// A second session is already open on this register (enforced by the one-open-session unique).
    SessionAlreadyOpen,

    // --- session close variance ------------------------------------------------
    /// The variance seam refused the booking (propagates the seam's code + message). Nothing has been
    /// written when this surfaces — the close rolls back entirely.
    VarianceRejected { code: String, message: String },

    // --- manager PIN -----------------------------------------------------------
    /// A privileged mutation was attempted without manager authorization.
    ManagerAuthRequired,
    /// No live PIN is set for that manager at this company.
    PinNotFound,
    /// The presented PIN does not match the stored hash.
    PinInvalid,
    /// The manager's PIN is locked: too many consecutive failures. Carries the unlock instant.
    PinLocked { until: chrono::DateTime<chrono::Utc> },
    /// Too many verification attempts from one source address inside the throttle window.
    PinThrottled,
    /// The chosen PIN does not satisfy the policy (digits + length window).
    WeakPin(&'static str),

    // --- restaurant seating ------------------------------------------------------
    /// The named dining table does not exist (in this tenant).
    TableNotFound(Uuid),
    /// The named dining table already holds a live DRAFT ticket — one draft per table; resume,
    /// settle, or move that ticket before opening a second one on the table. Carries the occupying
    /// ticket so the client can jump to it. (The DB partial unique is the backstop; this is the
    /// friendly typed refusal.)
    TableOccupied { pos_table_id: Uuid, draft_invoice_id: Uuid },

    // --- order-level discount ----------------------------------------------------
    /// The named discount master row does not exist (in this tenant) or is retired.
    DiscountNotFound(Uuid),
    /// The register is configured discount-off (`allow_discount = false`) — a ring naming a
    /// discount master is refused.
    DiscountNotAllowed,
    /// The discount master's stored percentage is unusable (over 100%). The master row is
    /// misconfigured; nothing was rung.
    DiscountInvalid(&'static str),
    Db(sqlx::Error),
}

impl PosError {
    pub fn code(&self) -> String {
        match self {
            PosError::EmptyDocument => "empty_document".into(),
            PosError::NegativeAmount => "negative_amount".into(),
            PosError::SessionNotOpen => "session_not_open".into(),
            PosError::SessionRegisterMismatch => "session_register_mismatch".into(),
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
            PosError::PartialReturnsNotImplemented => "partial_returns_not_implemented".into(),
            PosError::ProfileTaxTemplatesMissing(_) => "profile_tax_templates_missing".into(),
            PosError::TaxRejected { code, .. } => code.clone(),
            PosError::SessionClosedRescueRequired(_) => "session_closed_rescue_required".into(),
            PosError::SyncPartnerMismatch => "sync_partner_mismatch".into(),
            PosError::SyncSessionMismatch => "sync_session_mismatch".into(),
            PosError::RefundParentNotFound(_) => "refund_parent_not_found".into(),
            PosError::RefundLineageConflict => "refund_lineage_conflict".into(),
            PosError::InvalidTenderMethod(_) => "invalid_tender_method".into(),
            PosError::DuplicateClientUuid(_) => "duplicate_client_uuid".into(),
            PosError::SessionHasDraftOrders(_) => "session_has_draft_orders".into(),
            PosError::SessionHasUnpostedInvoices(_) => "session_has_unposted_invoices".into(),
            PosError::SessionAlreadyOpen => "session_already_open".into(),
            PosError::VarianceRejected { code, .. } => code.clone(),
            PosError::ManagerAuthRequired => "manager_authorization_required".into(),
            PosError::PinNotFound => "manager_pin_not_set".into(),
            PosError::PinInvalid => "manager_pin_invalid".into(),
            PosError::PinLocked { .. } => "manager_pin_locked".into(),
            PosError::PinThrottled => "manager_pin_throttled".into(),
            PosError::WeakPin(_) => "weak_pin".into(),
            PosError::TableNotFound(_) => "table_not_found".into(),
            PosError::TableOccupied { .. } => "table_occupied".into(),
            PosError::DiscountNotFound(_) => "discount_not_found".into(),
            PosError::DiscountNotAllowed => "discount_not_allowed".into(),
            PosError::DiscountInvalid(_) => "discount_invalid".into(),
            PosError::Db(_) => "internal_error".into(),
        }
    }
    pub fn http_status(&self) -> u16 {
        match self {
            PosError::ProfileNotFound(_)
            | PosError::InvoiceNotFound(_)
            | PosError::SessionNotFound(_)
            | PosError::TableNotFound(_)
            | PosError::DiscountNotFound(_) => 404,
            // Failed/absent manager credentials are authorization outcomes, not validation ones.
            PosError::ManagerAuthRequired | PosError::PinInvalid => 403,
            // Lockout + source throttling are rate-limit outcomes.
            PosError::PinLocked { .. } | PosError::PinThrottled => 429,
            // A table already holding a draft is a state conflict on a named resource (PSX-1 said
            // loudly, not silently).
            PosError::TableOccupied { .. } => 409,
            PosError::Db(_) => 500,
            _ => 422,
        }
    }
}
impl std::fmt::Display for PosError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PosError::BillingRejected { code, message }
            | PosError::PaymentRejected { code, message }
            | PosError::TaxRejected { code, message }
            | PosError::VarianceRejected { code, message } => write!(f, "{code}: {message}"),
            PosError::PinLocked { until } => write!(f, "manager_pin_locked until {until}"),
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

/// The constraint name of a unique violation, when it is one. More discriminating than [`is_dup`]:
/// the ticket table now carries several partial uniques (receipt number, sync uuid, one draft per
/// dining table), and the service maps each to its OWN typed refusal instead of guessing.
pub(super) fn dup_constraint(e: &sqlx::Error) -> Option<String> {
    let db = e.as_database_error()?;
    if !db.is_unique_violation() {
        return None;
    }
    db.constraint().map(|c| c.to_string())
}

/// Map a ticket-header unique violation to its typed refusal by constraint name, enriching the
/// one-draft-per-table collision with the occupying ticket (read on a FRESH connection — the
/// caller's transaction is aborted by the failed insert). Unmapped unique violations surface as the
/// raw DB error (`internal_error`) — an honest 500 for an index this module did not declare, never
/// a mislabeled 4xx.
pub(super) async fn map_invoice_dup(
    e: sqlx::Error,
    receipt_number: &str,
    client_uuid: Option<Uuid>,
    pos_table_id: Option<Uuid>,
    company_id: Uuid,
    invoices: &crate::infrastructure::persistence::PosInvoiceRepository,
    pool: &sqlx::PgPool,
    exclude_invoice_id: Option<Uuid>,
) -> PosError {
    match dup_constraint(&e).as_deref() {
        Some(c) if c.contains("receipt_number") => PosError::DuplicateNumber(receipt_number.to_string()),
        Some(c) if c.contains("client_uuid") => PosError::DuplicateClientUuid(client_uuid.unwrap_or_default()),
        Some(c) if c.contains("pos_table_id") => {
            let table = pos_table_id.unwrap_or_default();
            let draft = invoices
                .find_draft_on_table(pool, table, company_id, exclude_invoice_id)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            PosError::TableOccupied { pos_table_id: table, draft_invoice_id: draft }
        }
        _ => PosError::Db(e),
    }
}

/// The manager-PIN policy: every window is configuration, not code — a probe process must be able to
/// shorten the lockout without sleeping a production timeout, and a production deployment must be able
/// to tighten it. Defaults are deliberately short (the values below are what tests and local dev run
/// with) so the shipped library is usable out of the box; `from_env` reads overrides.
#[derive(Debug, Clone)]
pub struct PinPolicy {
    /// Consecutive failures before a manager's PIN locks.
    pub max_attempts: u32,
    /// How long a lockout holds, in seconds.
    pub lockout_secs: i64,
    /// Shortest acceptable PIN length (digits).
    pub min_digits: usize,
    /// Longest acceptable PIN length (digits).
    pub max_digits: usize,
    /// Per-source-address verification window, in seconds.
    pub ip_window_secs: i64,
    /// Verifications allowed per source address inside `ip_window_secs` (successful or not).
    pub ip_max_attempts: u32,
}

impl Default for PinPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            lockout_secs: 15,
            min_digits: 4,
            max_digits: 8,
            ip_window_secs: 60,
            ip_max_attempts: 20,
        }
    }
}

impl PinPolicy {
    /// Read overrides from the environment, falling back to [`PinPolicy::default`] per variable:
    /// `POS_PIN_MAX_ATTEMPTS`, `POS_PIN_LOCKOUT_SECS`, `POS_PIN_MIN_DIGITS`, `POS_PIN_MAX_DIGITS`,
    /// `POS_PIN_IP_WINDOW_SECS`, `POS_PIN_IP_MAX_ATTEMPTS`.
    pub fn from_env() -> Self {
        let d = Self::default();
        // Each override parses in its own field's type; a bad or missing value falls back per-field
        // (never a zeroed window — a typo cannot silently disable the lockout).
        let get = |k: &str| std::env::var(k).ok();
        Self {
            max_attempts: get("POS_PIN_MAX_ATTEMPTS").and_then(|v| v.trim().parse().ok()).unwrap_or(d.max_attempts),
            lockout_secs: get("POS_PIN_LOCKOUT_SECS").and_then(|v| v.trim().parse().ok()).unwrap_or(d.lockout_secs),
            min_digits: get("POS_PIN_MIN_DIGITS").and_then(|v| v.trim().parse().ok()).unwrap_or(d.min_digits),
            max_digits: get("POS_PIN_MAX_DIGITS").and_then(|v| v.trim().parse().ok()).unwrap_or(d.max_digits),
            ip_window_secs: get("POS_PIN_IP_WINDOW_SECS").and_then(|v| v.trim().parse().ok()).unwrap_or(d.ip_window_secs),
            ip_max_attempts: get("POS_PIN_IP_MAX_ATTEMPTS").and_then(|v| v.trim().parse().ok()).unwrap_or(d.ip_max_attempts),
        }
    }
}

/// The repositories are held behind `Arc` only so this service stays `Clone` (its HTTP surface clones
/// it per request) — `GenericCrudRepository` is not itself `Clone`. They are stateless handles over the
/// same pool; the `Arc` carries no shared mutable state. The PIN attempt ring is the one piece of
/// genuinely shared mutable state — `Mutex`, never held across an `.await`.
#[derive(Clone)]
pub struct PosWriteService {
    pub(super) db_pool: PgPool,
    pub(super) sink: Arc<dyn PosEventSink>,
    /// When set, `add_tender` stages `PosTenderCompleted` into this outbox schema
    /// **inside the tender's transaction** (durable) so a relay can recognise the sale even if the
    /// process dies between commit and the in-process `sink` spawn. `None` (default) preserves the
    /// historical fire-and-forget behaviour. Opt in via [`Self::with_outbox`].
    pub(super) outbox_schema: Option<String>,
    pub(super) invoices: Arc<PosInvoiceRepository>,
    pub(super) items: Arc<PosInvoiceItemRepository>,
    pub(super) payments: Arc<PosPaymentRepository>,
    pub(super) profiles: Arc<PosProfileRepository>,
    pub(super) openings: Arc<PosOpeningEntryRepository>,
    pub(super) closings: Arc<PosClosingEntryRepository>,
    pub(super) movements: Arc<PosCashMovementRepository>,
    /// Manager-PIN credentials (argon2 hashes + attempt counters + lockout).
    pub(super) pins: Arc<PosManagerPinRepository>,
    /// Dining tables (restaurant lane: seating validation on ring/sync).
    pub(super) tables: Arc<PosTableRepository>,
    /// Order-level discount masters (restaurant lane: the server-side discount RATE source).
    pub(super) discounts: Arc<PosDiscountRepository>,
    /// The PIN windows above; [`Self::with_pin_policy`] overrides the defaults.
    pub(super) pin_policy: PinPolicy,
    /// Rolling per-source-address verification ring: `ip -> (window_start_unix, attempts)`. A process
    /// -local guard in front of the persisted counters (which key on the manager identity).
    pub(super) ip_attempts: Arc<Mutex<HashMap<String, (i64, u32)>>>,
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
            pins: Arc::new(PosManagerPinRepository::new(db_pool.clone())),
            tables: Arc::new(PosTableRepository::new(db_pool.clone())),
            discounts: Arc::new(PosDiscountRepository::new(db_pool.clone())),
            db_pool,
            sink,
            outbox_schema: None,
            pin_policy: PinPolicy::default(),
            ip_attempts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Opt into durable, outbox-backed `PosTenderCompleted` staging: `add_tender` will insert the
    /// event into `{schema}.outbox_events` inside the tender transaction (so it commits atomically
    /// with the tender). A composing service runs `backbone_outbox::runner` to drain the queue and
    /// recognise the sale — surviving a crash between commit and the in-process `sink` spawn.
    /// `None`/unset keeps the fire-and-forget behaviour. The schema must already exist (the service
    /// runs `backbone_outbox::outbox::migrate(pool, schema)` at startup).
    pub fn with_outbox(mut self, schema: impl Into<String>) -> Self {
        self.outbox_schema = Some(schema.into());
        self
    }

    /// Override the manager-PIN windows (lockout, throttling, strength). Callers that want the
    /// environment to decide can pass [`PinPolicy::from_env`].
    pub fn with_pin_policy(mut self, policy: PinPolicy) -> Self {
        self.pin_policy = policy;
        self
    }


    // ---- session ------------------------------------------------------------

    pub async fn open_session(&self, s: NewSession) -> Result<Uuid, PosError> {
        // RLS scope (ADR-0008): bind this call to its own company for the whole body, so every query
        // runs with `app.company_id` set — via the request-dedicated connection under HTTP, or a
        // per-statement scope for non-request callers (jobs). The explicit `company_id` binds below
        // stay as defense-in-depth. This is the pattern every custom write service should follow.
        let company = s.company_id;
        company_scope::with_company_scope(Some(company), async move {
            // The register must exist in THIS tenant before anything is written: without the
            // check a session opens against a uuid the company does not own (or that does not
            // exist at all), and the close later fails when its register joins resolve nothing.
            // The lookup runs under the same fence as the insert, so a uuid owned by another
            // tenant reads as plain absence — the refusal cannot distinguish whose it is, and
            // must not (that distinction would be a cross-tenant oracle).
            let profile_known = self
                .profiles
                .exists(&self.db_pool, s.pos_profile_id, s.company_id)
                .await
                .map_err(PosError::from)?;
            if !profile_known {
                return Err(PosError::ProfileNotFound(s.pos_profile_id));
            }
            let id = Uuid::new_v4();
            let opening = serde_json::Value::Array(s.opening_balances.iter().map(|(m, a)| {
                serde_json::json!({ "method": m, "amount": a.to_string() })
            }).collect());
            let r = self.openings.insert_opening_entry(&self.db_pool, &NewOpeningEntryRow {
                id,
                company_id: s.company_id,
                pos_profile_id: s.pos_profile_id,
                branch_id: s.branch_id,
                cashier_party_id: s.cashier_party_id,
                opened_at: s.opened_at,
                opening_balances: opening,
            }).await;
            if let Err(e) = r {
                // The one-open-session-per-register partial unique (on (company_id, pos_profile_id)
                // — the register slot is the tenant's own) is the DB arm; map its violation to the
                // typed refusal instead of an internal error. Both the current company-scoped index
                // name and the older profile-only one carry the "pos_profile_id" fragment this
                // matcher keys on, so the mapping holds on databases either side of the re-key.
                return Err(match dup_constraint(&e).as_deref() {
                    Some(c) if c.contains("pos_profile_id") => PosError::SessionAlreadyOpen,
                    _ => e.into(),
                });
            }
            self.sink.publish(PosEvent::PosSessionOpened(PosSessionOpened {
                opening_entry_id: id, pos_profile_id: s.pos_profile_id, company_id: s.company_id,
            }));
            Ok(id)
        }).await
    }
}
