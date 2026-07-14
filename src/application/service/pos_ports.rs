//! Outbound orchestration ports (hand-authored, user-owned) — how POS drives billing + payment.
//!
//! POS owns NO GL path and does NOT import billing/payment. On `recognize_sale` it hands a serialized
//! request to a `BillingPort` (raise + post the real Sales Invoice — revenue) and a `PaymentPort`
//! (settle the tender). A composition layer implements both over the real `backbone-billing` /
//! `backbone-payment` services; the shipped POS library has ZERO normal Cargo edge to either. The
//! ports are the wire contract — the same envelope+ACL discipline every seam uses, here for two
//! downstream emitters instead of one.

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

/// The request POS hands billing to CREDIT-NOTE a sale (reverse the revenue) on a return.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreditNoteRequest {
    pub company_id: Uuid,
    /// The billing Sales Invoice to credit-note (`Dr Revenue · Cr A/R`, invoice → cancelled).
    pub invoice_ref: Uuid,
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
