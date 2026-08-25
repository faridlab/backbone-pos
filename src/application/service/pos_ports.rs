//! Outbound orchestration ports (hand-authored, user-owned) — how POS drives billing + payment +
//! inventory, resolves document-grade tax, and books the session-close variance.
//!
//! POS owns NO GL path and does NOT import billing/payment/tax. On `recognize_sale` it hands a
//! serialized request to a `BillingPort` (raise + post the real Sales Invoice — revenue) and a
//! `PaymentPort` (settle the tender). A composition layer implements both over the real
//! `backbone-billing` / `backbone-payment` services; the shipped POS library has ZERO normal Cargo
//! edge to either. The ports are the wire contract — the same envelope+ACL discipline every seam
//! uses, here for the downstream emitters instead of one. The same posture carries the newer seams:
//! `PosTaxComputePort` (the register's templates resolved document-grade) and `PosCashVariancePort`
//! (the one new GL surface a session close produces).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One sale line for the invoice request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SaleLine {
    pub item_id: Uuid,
    pub revenue_account_id: Uuid,
    pub quantity: Decimal,
    pub unit_price: Decimal,
}

/// The request POS hands billing: raise the Sales Invoice for this sale + post its revenue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SaleInvoiceRequest {
    pub company_id: Uuid,
    pub customer_id: Uuid,
    pub currency: String,
    /// The POS ticket id (the invoice's `source` reference).
    pub source_pos_id: Uuid,
    pub receivable_account_id: Uuid,
    pub posting_date: chrono::NaiveDate,
    pub lines: Vec<SaleLine>,
    /// Supplied PPN (tax) total + the account it credits (0 / None for a tax-free sale).
    pub tax_total: Decimal,
    pub tax_account_id: Option<Uuid>,
    /// The PPN rate that produced `tax_total` (e.g. 0.11 for 11%); 0 for a tax-free sale. Informational
    /// for the billing tax line + the receipt — billing books GL off `tax_total`, not this.
    pub tax_rate: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvoiceAck {
    pub invoice_id: Uuid,
    pub journal_id: Uuid,
    pub grand_total: Decimal,
}

/// The request POS hands payment: settle `amount` against the raised invoice (cash/card tender).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettlementRequest {
    pub company_id: Uuid,
    pub customer_id: Uuid,
    pub currency: String,
    /// The billing invoice being settled (from the `InvoiceAck`).
    pub invoice_ref: Uuid,
    pub bank_account_id: Uuid,
    pub party_account_id: Uuid,
    pub posting_date: chrono::NaiveDate,
    pub amount: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettlementAck {
    pub payment_id: Uuid,
    pub journal_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PosRejected {
    pub code: String,
    pub message: String,
}

/// A line to credit on a partial return — mirrors the [`SaleLine`] shape POS handed billing on the
/// sale (`qty · unit_price` is the line's net; `item_id` is the logical catalog FK).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreditLine {
    pub item_id: Uuid,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    pub revenue_account_id: Uuid,
}

/// Optional partial-credit payload for [`CreditNoteRequest`].
///
/// **Forward-compatible seam (council 2026-07-26, #5 slice):** added now so POS can request a
/// line-level credit note the moment `backbone-billing`'s `credit_note` / `reverse_sales_invoice`
/// honors a line subset — without a later wire ABI break. `amount`, when set, overrides the summed
/// line total (e.g. a header-level partial). POS does not construct this yet (full-ticket returns
/// only — ADR-001 park); `return_sale` gates `Some(_)` as `PartialReturnsNotImplemented`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartialCredit {
    /// The subset of sale lines to reverse (item + qty). Empty = amount-only partial.
    pub lines: Vec<CreditLine>,
    /// Optional override for the total to credit; otherwise the sum of `lines`.
    pub amount: Option<Decimal>,
}

/// The request POS hands billing to CREDIT-NOTE a sale (reverse the revenue) on a return.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreditNoteRequest {
    pub company_id: Uuid,
    /// The billing Sales Invoice to credit-note (`Dr Revenue · Cr A/R`, invoice → cancelled).
    pub invoice_ref: Uuid,
    /// Optional line-level / amount partial. `None` = whole-invoice credit note (today's behavior).
    /// `#[serde(default)]` keeps the wire backward-compatible with adapters that don't send it.
    #[serde(default)]
    pub partial: Option<PartialCredit>,
}

/// The request POS hands payment to REFUND a settled sale (reverse the tender) on a return.
///
/// Carries the `payment_id` of the settling `PaymentEntry` (ADR-001 addendum 2026-07-14): the refund
/// contract must be self-contained so a `PaymentPort` impl can reverse the tender **without reading
/// another module's private tables**. Before this field, the seam adapter resolved the payment by a
/// cross-schema `SELECT ... FROM payment.payment_allocations`, which only works when POS, billing,
/// payment, and accounting co-locate in one database — it is unsatisfiable over a bus. POS persists the
/// settlement `payment_id` on the ticket at recognition (see `PosInvoice.payment_entry_id`) and hands it
/// back here, which also discharges the ADR-001 `settle`-idempotency park (payment_id becomes a durable
/// skip-gate).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RefundRequest {
    pub company_id: Uuid,
    /// The billing invoice whose settlement is being reversed (`Dr A/R · Cr Cash`).
    pub invoice_ref: Uuid,
    /// The `PaymentEntry` that settled this sale — the tender being reversed. Nil only for a legacy
    /// ticket recognised before `payment_entry_id` was persisted (pre-2026-07-14).
    pub payment_id: Uuid,
    pub amount: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReversalAck {
    pub journal_id: Uuid,
}

/// Billing seam: a composing service implements this over billing's Sales Invoice create+post and its
/// credit-note (`reverse_sales_invoice`).
#[async_trait::async_trait]
pub trait BillingPort: Send + Sync {
    async fn raise_and_post(&self, req: &SaleInvoiceRequest) -> Result<InvoiceAck, PosRejected>;
    async fn credit_note(&self, req: &CreditNoteRequest) -> Result<ReversalAck, PosRejected>;
}

/// Payment seam: a composing service implements this over payment's create+post + apply_settlement,
/// and its refund (`reverse_payment`).
#[async_trait::async_trait]
pub trait PaymentPort: Send + Sync {
    async fn settle(&self, req: &SettlementRequest) -> Result<SettlementAck, PosRejected>;
    async fn refund(&self, req: &RefundRequest) -> Result<ReversalAck, PosRejected>;
}

/// One sale line to issue out of stock (item + quantity moved off the shelf).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StockIssueLine {
    pub item_id: Uuid,
    pub quantity: Decimal,
}

/// The request POS hands inventory to ISSUE (decrement) stock for a recognised sale — an outward
/// Delivery Note. Inventory relieves on-hand and books `Dr COGS · Cr Inventory` at the item's
/// moving-average cost. Driven only when the register has a warehouse + COGS/inventory accounts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StockIssueRequest {
    pub company_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub customer_id: Uuid,
    /// The POS ticket id (the delivery note's `source` reference).
    pub source_pos_id: Uuid,
    pub warehouse_id: Uuid,
    pub cogs_account_id: Uuid,
    pub inventory_account_id: Uuid,
    pub posting_date: chrono::NaiveDate,
    pub lines: Vec<StockIssueLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StockIssueAck {
    pub delivery_note_id: Uuid,
    pub journal_id: Option<Uuid>,
}

/// Inventory seam: a composing service implements this over inventory's delivery-note issue (create +
/// submit). Zero normal Cargo edge to inventory — the DTOs are the wire contract.
#[async_trait::async_trait]
pub trait InventoryPort: Send + Sync {
    async fn issue(&self, req: &StockIssueRequest) -> Result<StockIssueAck, PosRejected>;
}

// ---------------------------------------------------------------------------
// Document-grade tax compute
// ---------------------------------------------------------------------------

/// One taxed line handed to the tax compute: `line_ref` identifies the TICKET line (a POS-side
/// correlation id, not a persisted id), `template_id` is the register-configured template applying to
/// it, and `net_amount` is the line's tax-excluded net (quantity × unit_price − discount) in the
/// currency's smallest accounted unit. When a register carries several templates, POS emits one
/// `PosTaxLineIn` per (line, template) pair — the implementer expands them the same way the tax
/// engine expands its own input lines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PosTaxLineIn {
    pub line_ref: Uuid,
    pub template_id: Uuid,
    pub net_amount: Decimal,
}

/// Whether the document being taxed is a sale or a refund of one — the repartition family (and the
/// sign of withholding components) differs between them.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PosTaxDocumentType {
    Invoice,
    Refund,
}

/// The request POS hands the document-grade tax compute: the register's templates applied to the
/// ticket's line nets, on the ticket's date, for this company.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PosTaxComputeRequest {
    pub company_id: Uuid,
    pub document_type: PosTaxDocumentType,
    pub on_date: chrono::NaiveDate,
    pub lines: Vec<PosTaxLineIn>,
}

/// One routed tax split of one ticket line. `account_id` is the posting account (the cash-basis
/// transition account when the template defers — `real_account_id` then carries the account the
/// amount flips to as payments reconcile).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PosTaxComponent {
    pub line_ref: Uuid,
    pub template_id: Uuid,
    pub account_id: Option<Uuid>,
    pub real_account_id: Option<Uuid>,
    pub rate: Decimal,
    pub tax_amount: Decimal,
    pub description: Option<String>,
}

/// The document-grade result. `net_amounts` is keyed per ticket line (`line_ref`) and OVERWRITES
/// POS's own per-line rounding — a globally-rounding tax policy redistributes per-line cents so the
/// journal balances, and callers MUST adopt these nets as the line nets. `excluded_total` is the
/// Σ of those nets, `tax_total` the Σ of the signed components, `included_total` their sum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PosTaxComputeResult {
    pub net_amounts: Vec<(Uuid, Decimal)>,
    pub components: Vec<PosTaxComponent>,
    pub excluded_total: Decimal,
    pub tax_total: Decimal,
    pub included_total: Decimal,
}

/// Tax seam: a composing service implements this over the tax module's document engine
/// (`calculate_document`). POS holds ZERO normal Cargo edge to the tax module — the template refs
/// live on the register profile, and this port is the only thing that resolves them. A register with
/// no configured templates never reaches this port: the ring path refuses first (fail-closed).
#[async_trait::async_trait]
pub trait PosTaxComputePort: Send + Sync {
    async fn compute_document(&self, req: &PosTaxComputeRequest) -> Result<PosTaxComputeResult, PosRejected>;
}

// ---------------------------------------------------------------------------
// Session-close cash variance
// ---------------------------------------------------------------------------

/// The direction of a booked drawer variance at session close.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CashVarianceDirection {
    /// The drawer holds MORE than the journals say (`Dr Cash · Cr difference account`).
    Over,
    /// The drawer holds LESS than the journals say (`Dr difference account · Cr Cash`).
    Short,
}

/// The request POS hands the variance seam when a session close counts a non-zero drawer
/// difference: book it against the register's cash account and its difference/write-off account.
/// POS posts no GL itself — this is the ONE new GL surface a close produces (per-ticket posting
/// stays the posting path).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CashVarianceRequest {
    pub company_id: Uuid,
    /// The session being closed and its closing entry (source references for the journal).
    pub opening_entry_id: Uuid,
    pub closing_entry_id: Uuid,
    pub posting_date: chrono::NaiveDate,
    pub currency: String,
    /// The register's cash account (the drawer side of the correction).
    pub cash_account_id: Uuid,
    /// The register's difference / write-off account (the absorption side).
    pub difference_account_id: Uuid,
    /// Absolute variance magnitude; `direction` carries the sign.
    pub amount: Decimal,
    pub direction: CashVarianceDirection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CashVarianceAck {
    pub journal_id: Uuid,
}

/// Cash-variance seam: a composing service implements this over its ledger. The booking must be
/// idempotent on `opening_entry_id` — a session closes exactly once (the one-open-session unique plus
/// the close's open-state guard), so one session can never legitimately book two variances; a retried
/// close whose first attempt died between booking and commit must REUSE the journal, not double it.
/// `closing_entry_id` is the source reference for the journal's traceability.
#[async_trait::async_trait]
pub trait PosCashVariancePort: Send + Sync {
    async fn book_cash_variance(&self, req: &CashVarianceRequest) -> Result<CashVarianceAck, PosRejected>;
}
