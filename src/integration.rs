//! Stable integration surface for composing services.
//!
//! This module is the **deliberate, published API** a consumer depends on. It re-exports exactly
//! the types and functions a composing service (e.g. `serpa-posman-service`) needs to mount POS,
//! implement the outbound ports, and drive recognition — gathered under one namespace so that the
//! crate's internal DDD layer tree (`application::service::...`, `presentation::http::...`) is free
//! to move on regeneration without breaking consumers.
//!
//! **Consumers should depend only on `backbone_pos::integration::{...}`.** The deep paths are
//! internal; reaching into them couples you to the layer structure (council 2026-07-26, Contract
//! Seat — `docs/council/2026-07-26-module-backbone-pos-maturity.md`).
//!
//! ## Stability promise
//! - **Additive** changes (new re-exports, new items) are non-breaking.
//! - **Removals or renames** of items already exported here require a semver bump.
//! - The internals behind these aliases are NOT promised stable — they may change anytime.
//!
//! ## What a consumer wires
//! ```ignore
//! use backbone_pos::integration::*;
//!
//! let pos = PosModule::builder().with_database(pool.clone()).build()?;
//! // implement the outbound ports over your real billing/payment/inventory
//! let billing: Arc<dyn BillingPort> = /* your adapter */;
//! let payment: Arc<dyn PaymentPort> = /* your adapter */;
//! let inventory: Option<Arc<dyn InventoryPort>> = /* your adapter */;
//! // subscribe to tenders → drive recognition exactly once
//! let sink = RecognitionSink::new(pool.clone(), billing, payment, inventory);
//! // mount the guarded, tenant-fenced, outbox-durable surface
//! let router = create_guarded_pos_routes_with_outbox(&pos, pool, tenant_verifier, sink, schema);
//! ```

// --- The module + builder -----------------------------------------------------
pub use crate::PosModule;
pub use crate::PosModuleBuilder;

// --- Guarded HTTP surface (route composers) -----------------------------------
// Read + validated writes; the production surface. NO generic CRUD (that's `PosModule::all_crud_routes`).
pub use crate::presentation::http::{
    create_guarded_pos_routes, create_guarded_pos_routes_with_outbox,
    create_guarded_pos_priced_route, create_guarded_pos_priced_route_with_outbox,
};

// --- Tenant / company auth (the JWT `CompanyContext` the guarded surface reads) -
// `Company*` are the canonical names (ADR-0005). `Tenant*` are deprecated aliases kept for back-compat.
pub use crate::presentation::http::{company_auth, CompanyClaims, CompanyContext, CompanyVerifier};
#[allow(deprecated)]
pub use crate::presentation::http::{tenant_auth, TenantClaims, TenantContext, TenantVerifier};

// --- Outbound ports: the wire contract a consumer implements ------------------
// POS posts no GL — it drives billing (revenue) + payment (settlement) + inventory (stock issue)
// through these traits. The shipped library has ZERO normal Cargo edges to those modules.
pub use crate::application::service::{
    BillingPort, PaymentPort, PosRejected,
    // billing leg
    SaleInvoiceRequest, SaleLine, InvoiceAck, CreditNoteRequest, ReversalAck,
    // payment leg
    SettlementRequest, SettlementAck, RefundRequest,
};
pub use crate::application::service::pos_ports::{InventoryPort, StockIssueRequest, StockIssueAck};

// --- Server-authoritative cart pricing (promo) port ---------------------------
pub use crate::application::service::{
    CartPricingPort, CartPriceRequest, CartPriceLine, CartPricingError,
    PricedCart, PricedCartLine, PricedRewardLine,
};

// --- The write service: open / ring / tender / recognize / return / drawer -----
pub use crate::application::service::PosWriteService;
// Vocabulary a consumer passes in / receives (kept here so callers don't reach into the service tree).
pub use crate::application::service::{
    NewSession, NewSale, NewSaleLine, NewCartSale, CartSaleLine, NewClose,
    TenderOutcome, RecognizeOutcome, ReturnOutcome, CloseOutcome, MethodRecon, PosError,
};

// --- Events + the sink a consumer subscribes to drive recognition -------------
// `PosTenderCompleted` is the recognition trigger: a composing service subscribes and calls
// `PosWriteService::recognize_sale` on it. The library does NOT call recognize internally.
pub use crate::application::service::{
    PosEvent, PosEventSink, LoggingSink,
    PosInvoicePaid, PosInvoiceReturned, PosSessionClosed, PosSessionOpened,
};
pub use crate::application::service::pos_events::PosTenderCompleted;
