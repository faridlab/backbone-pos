// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

pub mod pos_events;
pub mod pos_ports;
pub mod pos_cart_pricing;
pub mod pos_write_service;
// The write surface, chunked: each is an `impl PosWriteService` block over the vocabulary that stays
// in `pos_write_service` (so the `pos_write_service::{NewSale, ...}` import paths are unchanged).
pub mod pos_sale;
pub mod pos_tender;
pub mod pos_recognition;
pub mod pos_drawer;
pub mod pos_receipt;
// The server-owned ticket computation (document-grade tax + register cash rounding), the offline
// replay verb, and the manager-PIN credential path.
pub mod pos_compute;
pub mod pos_sync;
pub mod pos_manager_pin;
pub mod pos_discount;
pub mod pos_session_alert;
pub use pos_events::{
    LoggingSink, PosEvent, PosEventSink, PosInvoicePaid, PosInvoiceReturned, PosSessionClosed,
    PosSessionOpened,
};
pub use pos_ports::{
    BillingPort, CashVarianceAck, CashVarianceDirection, CashVarianceRequest, CreditNoteRequest,
    InvoiceAck, PaymentPort, PosCashVariancePort, PosRejected, PosTaxComponent, PosTaxComputePort,
    PosTaxComputeRequest, PosTaxComputeResult, PosTaxDocumentType, PosTaxLineIn, RefundRequest,
    ReversalAck, SaleInvoiceRequest, SaleLine, SettlementAck, SettlementRequest,
};
pub use pos_cart_pricing::{
    CartPriceLine, CartPriceRequest, CartPricingError, CartPricingPort, PricedCart, PricedCartLine,
    PricedRewardLine,
};
pub use pos_write_service::{
    CartSaleLine, CloseOutcome, ManagerAuth, MethodRecon, NewCartSale, NewClose, NewSale, NewSaleLine,
    NewSession, NewSyncSale, PinPolicy, PosError, PosWriteService, RecognizeOutcome, ReturnOutcome,
    SyncAction, SyncOutcome, SyncSaleLine, SyncTender, TenderOutcome, TicketTotals, VarianceBooking,
};
