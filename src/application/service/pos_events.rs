//! POS domain events (hand-authored, user-owned) — the public extension surface.
//!
//! POS posts NO GL itself; `PosInvoicePaid` (carrying the billing invoice + payment it drove) is the
//! hook a loyalty/analytics consumer subscribes to, and `PosSessionClosed` carries the drawer
//! reconciliation result. The retail path reuses billing (revenue) + payment (settlement) as the GL
//! emitters — POS is the aggregator over them.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A cashier session opened.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PosSessionOpened {
    pub opening_entry_id: Uuid,
    pub pos_profile_id: Uuid,
    pub company_id: Uuid,
}

/// A sale was fully tendered + recognised — POS drove billing (a real Sales Invoice + revenue post)
/// and payment (tender settlement). Carries both so a consumer needn't call back into POS.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PosInvoicePaid {
    pub pos_invoice_id: Uuid,
    pub company_id: Uuid,
    pub grand_total: Decimal,
    pub rounded_total: Decimal,
    pub billing_invoice_id: Uuid,
    pub payment_id: Uuid,
}

/// A cashier session was closed + reconciled (drawer counted vs expected per method).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PosSessionClosed {
    pub closing_entry_id: Uuid,
    pub opening_entry_id: Uuid,
    pub company_id: Uuid,
    pub difference_total: Decimal,
}

/// A sale was returned — POS drove billing (credit note) + payment (refund), so revenue, cash, and
/// A/R all net back to zero. Carries the original + the return ticket for downstream (loyalty claw-back,
/// analytics).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PosInvoiceReturned {
    pub pos_invoice_id: Uuid,
    pub return_ticket_id: Uuid,
    pub company_id: Uuid,
    pub billing_invoice_id: Uuid,
    pub amount: Decimal,
}

/// The POS domain-event union.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PosEvent {
    PosSessionOpened(PosSessionOpened),
    PosInvoicePaid(PosInvoicePaid),
    PosSessionClosed(PosSessionClosed),
    PosInvoiceReturned(PosInvoiceReturned),
}

/// Sink for POS domain events. Fire-and-forget; a real adapter wires a bus, tests record.
pub trait PosEventSink: Send + Sync {
    fn publish(&self, event: PosEvent);
}

/// Default sink — emits structured tracing events.
pub struct LoggingSink;

impl PosEventSink for LoggingSink {
    fn publish(&self, event: PosEvent) {
        tracing::info!(target: "pos.events", ?event, "pos domain event");
    }
}
