//! Guarded route composition — the RECOMMENDED way to mount the POS module.
//!
//! Hand-authored (user-owned). Read documents + **validated writes** (open session / ring sale / sync
//! an offline ticket / add tender / close session / manager-PIN credentials); generic CRUD is NOT
//! mounted, so a caller cannot write a ticket with inconsistent totals or reopen a closed session.
//! `recognize_sale` drives billing + payment through ports (a composition layer), so it is
//! service/job-driven, not an HTTP route.
//!
//! Route bases carry no module prefix — the module mounts under its own scope (the composing service
//! nests it at `/api/v1/pos` or equivalent): `/sessions`, `/sales`, `/tenders`, `/cash-movements`,
//! `/receipts`, plus the newer `/sales/sync` (offline replay) and `/manager-pins` (credential verbs).
//!
//! The composing service supplies the port implementations the guarded surface needs:
//! a `PosTaxComputePort` (document-grade tax — every ring), a `PosCashVariancePort` (session-close
//! GL correction), and the `BillingPort`/`PaymentPort` pair (only the offline REFUND replay drives
//! them, through the same reversal pair `return_sale` uses).

use std::sync::Arc;

use axum::{extract::{Path, State}, http::StatusCode, middleware::from_fn_with_state, response::IntoResponse, routing::{get, post}, Json, Router};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::application::service::pos_cart_pricing::CartPricingPort;
use crate::application::service::pos_events::PosEventSink;
use crate::application::service::pos_ports::{BillingPort, PaymentPort, PosCashVariancePort, PosTaxComputePort};
use crate::application::service::pos_write_service::{
    CartSaleLine, ManagerAuth, NewCartSale, NewCashMovement, NewClose, NewSale, NewSaleLine,
    NewSession, NewSyncSale, PosError, PosWriteService, SyncAction, SyncSaleLine, SyncTender,
};
use crate::application::service::pos_manager_pin::SetPin;
use crate::PosModule;

use backbone_auth::company::{company_auth, CompanyContext, CompanyVerifier};
use super::{
    create_pos_discount_read_routes, create_pos_floor_plan_read_routes, create_pos_invoice_read_routes,
    create_pos_opening_entry_read_routes, create_pos_profile_read_routes, create_pos_table_read_routes,
};

#[derive(Debug, Serialize)]
struct ErrorBody { error: String, message: String }
#[derive(Debug, Serialize)]
struct IdResponse { id: Uuid }
fn err(e: PosError) -> axum::response::Response {
    let s = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (s, Json(ErrorBody { error: e.code(), message: e.to_string() })).into_response()
}

/// The best-effort source address of a privileged request — feeds the PIN throttle ring. Reads the
/// proxy hop list first (the guarded surface sits behind one in production), then the direct peer.
/// `None` skips the per-address budget (service-to-service callers have no address).
fn source_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|s| format!("ua:{}", utf8_prefix(s, 60)))
        })
}

/// Longest prefix of `s` no longer than `max` bytes that stops on a char boundary — a raw byte
/// slice panics when a multi-byte character straddles the cap, and this feeds the privileged
/// throttle path.
fn utf8_prefix(s: &str, max: usize) -> &str {
    let mut cut = s.len().min(max);
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    &s[..cut]
}

#[cfg(test)]
mod tests {
    use super::utf8_prefix;

    #[test]
    fn utf8_prefix_never_splits_a_character() {
        // 16 four-byte emoji = 64 bytes: a 60-byte cap lands exactly on a boundary.
        let emoji = "😀".repeat(16);
        assert_eq!(utf8_prefix(&emoji, 60).len(), 60);
        // A cap one byte past the boundary backs off to it instead of splitting an emoji.
        assert_eq!(utf8_prefix(&emoji, 61).len(), 60);
        // A two-byte character straddling the cap backs off whole.
        let tailed = "a".repeat(59) + "é";
        assert_eq!(utf8_prefix(&tailed, 60), "a".repeat(59).as_str());
        assert_eq!(utf8_prefix("short", 60), "short");
        assert_eq!(utf8_prefix("", 60), "");
    }
}

/// The write surface's shared state: the write service plus the port implementations the guarded
/// verbs drive (tax on every ring, variance at close, billing+payment on offline refund replays).
#[derive(Clone)]
struct WriteState {
    svc: Arc<PosWriteService>,
    tax: Arc<dyn PosTaxComputePort>,
    variance: Arc<dyn PosCashVariancePort>,
    billing: Arc<dyn BillingPort>,
    payment: Arc<dyn PaymentPort>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpeningBalanceBody { method: String, amount: Decimal }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenSessionBody {
    pos_profile_id: Uuid,
    cashier_party_id: Uuid,
    opened_at: chrono::NaiveDateTime,
    #[serde(default)] opening_balances: Vec<OpeningBalanceBody>,
}
async fn open_session(State(st): State<WriteState>, tenant: CompanyContext, Json(b): Json<OpenSessionBody>) -> axum::response::Response {
    // company_id / branch_id come from the authenticated principal, never the request body.
    let s = NewSession {
        company_id: tenant.company_id, pos_profile_id: b.pos_profile_id, branch_id: tenant.branch_id,
        cashier_party_id: b.cashier_party_id, opened_at: b.opened_at,
        opening_balances: b.opening_balances.into_iter().map(|o| (o.method, o.amount)).collect(),
    };
    match st.svc.open_session(s).await {
        Ok(id) => (StatusCode::CREATED, Json(IdResponse { id })).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaleLineBody {
    item_id: Uuid,
    #[serde(default)] revenue_account_id: Option<Uuid>,
    #[serde(default)] description: Option<String>,
    /// Course grouping for kitchen routing (1 = starter, 2 = main, …); absent = not grouped.
    #[serde(default)] course: Option<i32>,
    quantity: Decimal,
    unit_price: Decimal,
    #[serde(default)] discount_amount: Decimal,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RingSaleBody {
    pos_profile_id: Uuid,
    opening_entry_id: Uuid,
    #[serde(default)] customer_id: Option<Uuid>,
    /// Seat the draft at a dining table (restaurant lane). One draft per table is enforced
    /// server-side — a second draft on an occupied table is a typed 409, never a silent merge.
    #[serde(default)] pos_table_id: Option<Uuid>,
    /// Apply this tenant's order-level discount master to the whole ticket. The RATE is read from
    /// the master row server-side; a client-sent percentage has no field to land in.
    #[serde(default)] discount_id: Option<Uuid>,
    receipt_number: String,
    posting_at: chrono::NaiveDateTime,
    lines: Vec<SaleLineBody>,
}
async fn ring_sale(State(st): State<WriteState>, tenant: CompanyContext, Json(b): Json<RingSaleBody>) -> axum::response::Response {
    // Tenant from the principal — `ring_sale` scopes the session lookup by this company_id, so a token
    // for company A cannot ring against company B's opening_entry_id (the cross-tenant write is closed).
    // EVERY money field is server-derived: tax resolves through the register's templates via the tax
    // port, and cash rounding comes from the register's configuration — the body has no total fields
    // at all (a client can neither omit nor overstate the VAT or the pay-to total). The order-level
    // discount is likewise server-priced: only the master's id is accepted, never a rate.
    let sale = NewSale {
        company_id: tenant.company_id, pos_profile_id: b.pos_profile_id, opening_entry_id: b.opening_entry_id,
        branch_id: tenant.branch_id, customer_id: b.customer_id, pos_table_id: b.pos_table_id,
        discount_id: b.discount_id, receipt_number: b.receipt_number, posting_at: b.posting_at,
        lines: b.lines.into_iter().map(|l| NewSaleLine {
            item_id: l.item_id, revenue_account_id: l.revenue_account_id, description: l.description,
            course: l.course, quantity: l.quantity, unit_price: l.unit_price, discount_amount: l.discount_amount,
        }).collect(),
    };
    match st.svc.ring_sale(sale, &*st.tax).await {
        Ok(id) => (StatusCode::CREATED, Json(IdResponse { id })).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TenderBody {
    pos_invoice_id: Uuid,
    payment_method: String,
    amount: Decimal,
    #[serde(default)] reference_no: Option<String>,
}
async fn add_tender(State(st): State<WriteState>, _tenant: CompanyContext, Json(b): Json<TenderBody>) -> axum::response::Response {
    match st.svc.add_tender(b.pos_invoice_id, &b.payment_method, b.amount, b.reference_no).await {
        Ok(o) => (StatusCode::OK, Json(serde_json::json!({
            "paidTotal": o.paid_total, "changeDue": o.change_due, "fullyTendered": o.fully_tendered,
        }))).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CountedBody { method: String, amount: Decimal }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagerBody {
    employee_party_id: Uuid,
    pin: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseBody {
    opening_entry_id: Uuid,
    cashier_party_id: Uuid,
    closed_at: chrono::NaiveDateTime,
    #[serde(default)] counted: Vec<CountedBody>,
    /// Closing the till is privileged: the manager's PIN is verified server-side before anything is
    /// written (the close is the one POS verb that books a GL correction on its own account).
    manager: ManagerBody,
}
async fn close_session(State(st): State<WriteState>, headers: axum::http::HeaderMap, tenant: CompanyContext, Json(b): Json<CloseBody>) -> axum::response::Response {
    // company_id from the principal — close scopes the opening-entry lookup by it.
    let c = NewClose {
        company_id: tenant.company_id, opening_entry_id: b.opening_entry_id, cashier_party_id: b.cashier_party_id,
        closed_at: b.closed_at, counted: b.counted.into_iter().map(|x| (x.method, x.amount)).collect(),
        manager: ManagerAuth { employee_party_id: b.manager.employee_party_id, pin: b.manager.pin },
        source_ip: source_ip(&headers),
    };
    match st.svc.close_session(c, &*st.variance).await {
        Ok(o) => (StatusCode::OK, Json(serde_json::json!({
            "closingId": o.closing_id, "differenceTotal": o.difference_total,
            "byMethod": o.by_method.iter().map(|m| serde_json::json!({
                "method": m.method, "expected": m.expected, "counted": m.counted, "difference": m.difference,
            })).collect::<Vec<_>>(),
            "variance": match &o.variance {
                None => serde_json::Value::Null,
                Some(v) => serde_json::json!({ "journalId": v.journal_id, "amount": v.amount, "direction": match v.direction {
                    crate::application::service::pos_ports::CashVarianceDirection::Over => "over",
                    crate::application::service::pos_ports::CashVarianceDirection::Short => "short",
                }}),
            },
        }))).into_response(),
        Err(e) => err(e),
    }
}

// ---- offline sync (the replay verb) -------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncSaleLineBody {
    client_uuid: Uuid,
    item_id: Uuid,
    #[serde(default)] revenue_account_id: Option<Uuid>,
    #[serde(default)] description: Option<String>,
    #[serde(default)] course: Option<i32>,
    quantity: Decimal,
    unit_price: Decimal,
    #[serde(default)] discount_amount: Decimal,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncTenderBody {
    client_uuid: Uuid,
    method: String,
    amount: Decimal,
    #[serde(default)] reference_no: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncSaleBody {
    client_uuid: Uuid,
    pos_profile_id: Uuid,
    opening_entry_id: Uuid,
    #[serde(default)] rescue_opening_entry_id: Option<Uuid>,
    #[serde(default)] customer_id: Option<Uuid>,
    /// Seat/transfer the ticket to a dining table (restaurant lane): honored as the ticket's new
    /// seat on update; a table occupied by ANOTHER draft refuses with a typed 409.
    #[serde(default)] pos_table_id: Option<Uuid>,
    /// Order-level discount master (same server-priced contract as the online ring).
    #[serde(default)] discount_id: Option<Uuid>,
    posting_at: chrono::NaiveDateTime,
    #[serde(default)] lines: Vec<SyncSaleLineBody>,
    #[serde(default)] tenders: Vec<SyncTenderBody>,
    /// A refund replay names its parent ticket's client uuid — that makes the sync privileged
    /// (`manager` is then required and verified server-side).
    #[serde(default)] refund_of_client_uuid: Option<Uuid>,
    #[serde(default)] manager: Option<ManagerBody>,
}
async fn sync_from_ui(State(st): State<WriteState>, headers: axum::http::HeaderMap, tenant: CompanyContext, Json(b): Json<SyncSaleBody>) -> axum::response::Response {
    // No total field exists on this body by design: the server recomputes every money field from the
    // lines + tenders through the same compute core the online ring uses.
    let s = NewSyncSale {
        company_id: tenant.company_id,
        client_uuid: b.client_uuid,
        pos_profile_id: b.pos_profile_id,
        opening_entry_id: b.opening_entry_id,
        rescue_opening_entry_id: b.rescue_opening_entry_id,
        branch_id: tenant.branch_id,
        customer_id: b.customer_id,
        pos_table_id: b.pos_table_id,
        discount_id: b.discount_id,
        posting_at: b.posting_at,
        lines: b.lines.into_iter().map(|l| SyncSaleLine {
            client_uuid: l.client_uuid, item_id: l.item_id, revenue_account_id: l.revenue_account_id,
            description: l.description, course: l.course, quantity: l.quantity, unit_price: l.unit_price,
            discount_amount: l.discount_amount,
        }).collect(),
        tenders: b.tenders.into_iter().map(|t| SyncTender {
            client_uuid: t.client_uuid, method: t.method, amount: t.amount, reference_no: t.reference_no,
        }).collect(),
        refund_of_client_uuid: b.refund_of_client_uuid,
        manager: b.manager.map(|m| ManagerAuth { employee_party_id: m.employee_party_id, pin: m.pin }),
        source_ip: source_ip(&headers),
    };
    match st.svc.sync_from_ui(s, &*st.tax, &*st.billing, &*st.payment).await {
        Ok(o) => (StatusCode::OK, Json(serde_json::json!({
            "posInvoiceId": o.pos_invoice_id,
            "action": match o.action {
                SyncAction::Created => "created",
                SyncAction::Updated => "updated",
                SyncAction::ReplayFinalized => "replay_finalized",
            },
            "totals": {
                "netTotal": o.totals.net_total, "taxTotal": o.totals.tax_total,
                "grandTotal": o.totals.grand_total, "roundingAdjustment": o.totals.rounding_adjustment,
                "roundedTotal": o.totals.rounded_total, "paidTotal": o.totals.paid_total,
                "changeDue": o.totals.change_due,
            },
        }))).into_response(),
        Err(e) => err(e),
    }
}

// ---- manager PINs ---------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetPinBody {
    employee_party_id: Uuid,
    new_pin: String,
    /// Authority proof when the company already has credentials: the same manager's current PIN, or
    /// another manager's. Absent is valid only for the very first credential (bootstrap).
    #[serde(default)] current: Option<ManagerBody>,
}
async fn set_pin(State(st): State<WriteState>, headers: axum::http::HeaderMap, tenant: CompanyContext, Json(b): Json<SetPinBody>) -> axum::response::Response {
    let s = SetPin {
        company_id: tenant.company_id,
        employee_party_id: b.employee_party_id,
        new_pin: b.new_pin,
        current: b.current.map(|m| ManagerAuth { employee_party_id: m.employee_party_id, pin: m.pin }),
        source_ip: source_ip(&headers),
    };
    match st.svc.set_pin(s).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyPinBody { employee_party_id: Uuid, pin: String }
async fn verify_pin(State(st): State<WriteState>, headers: axum::http::HeaderMap, tenant: CompanyContext, Json(b): Json<VerifyPinBody>) -> axum::response::Response {
    match st.svc.verify_pin(tenant.company_id, b.employee_party_id, &b.pin, source_ip(&headers).as_deref()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(e),
    }
}

// ---- priced ring (the promo cart seam) ---------------------------------------
//
// `POST /sales/priced` rings a ticket whose per-line prices are RESOLVED by promo (server-side
// discounts + order rules + bundles) instead of taken from the client. The composing service supplies a
// `CartPricingPort` implemented over promo's `resolve_cart`. The client still sends a `listPrice` per
// line (no price master exists yet — base price is not server-owned), but the DISCOUNT it pays is
// server-authoritative: a client cannot fake a promo. Tax + rounding are server-owned as on the plain
// ring.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PricedSaleLineBody {
    item_id: Uuid,
    #[serde(default)] item_group_id: Option<Uuid>,
    #[serde(default)] brand_id: Option<Uuid>,
    #[serde(default)] revenue_account_id: Option<Uuid>,
    #[serde(default)] description: Option<String>,
    #[serde(default)] course: Option<i32>,
    list_price: Decimal,
    quantity: Decimal,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RingSalePricedBody {
    pos_profile_id: Uuid,
    opening_entry_id: Uuid,
    #[serde(default)] customer_id: Option<Uuid>,
    #[serde(default)] customer_group_id: Option<Uuid>,
    #[serde(default)] coupon_code: Option<String>,
    #[serde(default)] pos_table_id: Option<Uuid>,
    #[serde(default)] discount_id: Option<Uuid>,
    receipt_number: String,
    posting_at: chrono::NaiveDateTime,
    lines: Vec<PricedSaleLineBody>,
}

/// State for the priced route: the write state + the promo-backed pricer.
#[derive(Clone)]
struct PricedState {
    write: WriteState,
    pricing: Arc<dyn CartPricingPort>,
}

async fn ring_sale_priced(State(st): State<PricedState>, tenant: CompanyContext, Json(b): Json<RingSalePricedBody>) -> axum::response::Response {
    let cart = NewCartSale {
        company_id: tenant.company_id, pos_profile_id: b.pos_profile_id, opening_entry_id: b.opening_entry_id,
        branch_id: tenant.branch_id, customer_id: b.customer_id, customer_group_id: b.customer_group_id,
        coupon_code: b.coupon_code, pos_table_id: b.pos_table_id, discount_id: b.discount_id,
        receipt_number: b.receipt_number, posting_at: b.posting_at,
        lines: b.lines.into_iter().map(|l| CartSaleLine {
            item_id: l.item_id, item_group_id: l.item_group_id, brand_id: l.brand_id,
            revenue_account_id: l.revenue_account_id, description: l.description, course: l.course,
            list_price: l.list_price, quantity: l.quantity,
        }).collect(),
    };
    match st.write.svc.ring_sale_priced(cart, &*st.pricing, &*st.write.tax).await {
        Ok(id) => (StatusCode::CREATED, Json(IdResponse { id })).into_response(),
        Err(e) => err(e),
    }
}

/// Mount ONLY the promo-priced ring route (`POST /sales/priced`), authenticated. Merge this in
/// addition to `create_guarded_pos_routes` when the service has a promo-backed `CartPricingPort`.
pub fn create_guarded_pos_priced_route(
    pool: PgPool,
    verifier: CompanyVerifier,
    pricing: Arc<dyn CartPricingPort>,
    sink: Arc<dyn PosEventSink>,
    tax: Arc<dyn PosTaxComputePort>,
) -> Router {
    create_guarded_pos_priced_route_inner(pool, verifier, pricing, sink, tax, None)
}

/// Like [`create_guarded_pos_priced_route`] but opts the priced-ring write service into
/// outbox-backed `PosTenderCompleted` staging (durable recognition). See
/// [`create_guarded_pos_routes_with_outbox`] for the service-side setup the schema implies.
pub fn create_guarded_pos_priced_route_with_outbox(
    pool: PgPool,
    verifier: CompanyVerifier,
    pricing: Arc<dyn CartPricingPort>,
    sink: Arc<dyn PosEventSink>,
    tax: Arc<dyn PosTaxComputePort>,
    outbox_schema: String,
) -> Router {
    create_guarded_pos_priced_route_inner(pool, verifier, pricing, sink, tax, Some(outbox_schema))
}

fn create_guarded_pos_priced_route_inner(
    pool: PgPool,
    verifier: CompanyVerifier,
    pricing: Arc<dyn CartPricingPort>,
    sink: Arc<dyn PosEventSink>,
    tax: Arc<dyn PosTaxComputePort>,
    outbox_schema: Option<String>,
) -> Router {
    let mut svc = PosWriteService::with_sink(pool, sink);
    if let Some(schema) = outbox_schema {
        svc = svc.with_outbox(schema);
    }
    let write = WriteState {
        svc: Arc::new(svc),
        tax,
        // The priced ring never books variance or drives a refund — placeholders that satisfy the
        // state shape; the routes that would use them are not mounted here.
        variance: Arc::new(NoVariance),
        billing: Arc::new(NoBilling),
        payment: Arc::new(NoPayment),
    };
    let st = PricedState { write, pricing };
    Router::new()
        .route("/sales/priced", post(ring_sale_priced))
        .route_layer(from_fn_with_state(verifier, company_auth))
        .with_state(st)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CashMovementBody {
    pos_profile_id: Uuid,
    opening_entry_id: Uuid,
    cashier_party_id: Uuid,
    /// pay_in | pay_out | drop | no_sale
    movement_type: String,
    #[serde(default)] amount: Decimal,
    #[serde(default)] reason: Option<String>,
    moved_at: chrono::NaiveDateTime,
}
async fn record_cash_movement(State(st): State<WriteState>, tenant: CompanyContext, Json(b): Json<CashMovementBody>) -> axum::response::Response {
    // company_id from the authenticated principal; the session lookup is scoped by it.
    let m = NewCashMovement {
        company_id: tenant.company_id, pos_profile_id: b.pos_profile_id, opening_entry_id: b.opening_entry_id,
        cashier_party_id: b.cashier_party_id, movement_type: b.movement_type, amount: b.amount,
        reason: b.reason, moved_at: b.moved_at,
    };
    match st.svc.record_cash_movement(m).await {
        Ok(id) => (StatusCode::CREATED, Json(IdResponse { id })).into_response(),
        Err(e) => err(e),
    }
}

async fn x_report(State(st): State<WriteState>, tenant: CompanyContext, Path(opening_entry_id): Path<Uuid>) -> axum::response::Response {
    match st.svc.x_report(tenant.company_id, opening_entry_id).await {
        Ok(r) => (StatusCode::OK, Json(serde_json::json!({
            "openingEntryId": r.opening_entry_id,
            "grandTotal": r.grand_total,
            "invoiceCount": r.invoice_count,
            "byMethod": r.by_method.iter().map(|m| serde_json::json!({ "method": m.method, "expected": m.expected })).collect::<Vec<_>>(),
        }))).into_response(),
        Err(e) => err(e),
    }
}

async fn receipt(State(st): State<WriteState>, tenant: CompanyContext, Path(pos_invoice_id): Path<Uuid>) -> axum::response::Response {
    match st.svc.receipt(tenant.company_id, pos_invoice_id).await {
        Ok(r) => {
            let text = r.render_text();
            let mut body = serde_json::to_value(&r).unwrap_or_else(|_| serde_json::json!({}));
            if let Some(obj) = body.as_object_mut() {
                obj.insert("text".to_string(), serde_json::Value::String(text));
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => err(e),
    }
}

fn write_routes(st: WriteState, verifier: CompanyVerifier) -> Router {
    Router::new()
        .route("/sessions", post(open_session))
        .route("/sessions/close", post(close_session))
        .route("/sessions/:opening_entry_id/x-report", get(x_report))
        .route("/sales", post(ring_sale))
        .route("/sales/sync", post(sync_from_ui))
        .route("/tenders", post(add_tender))
        .route("/cash-movements", post(record_cash_movement))
        .route("/receipts/:pos_invoice_id", get(receipt))
        .route("/manager-pins", post(set_pin))
        .route("/manager-pins/verify", post(verify_pin))
        // Every write requires a valid Bearer token carrying a company_id claim; the layer inserts the
        // CompanyContext the handlers extract. Unauthenticated writes get 401 before touching the service.
        .route_layer(from_fn_with_state(verifier, company_auth))
        .with_state(st)
}

/// Mount the POS module: read documents + **authenticated** validated writes. Generic mutation is not
/// mounted; sale recognition (billing + payment handoff) is service/job-driven via the ports. The
/// `verifier` (built by the composing service from `JWT_SECRET`) authenticates every write and supplies
/// the tenant — callers do not send `company_id` in the body.
/// **Prefer this over `PosModule::all_crud_routes()` for any real deployment.**
///
/// `tax` resolves document-grade tax for every ring (implement over the tax module's
/// `calculate_document`); `variance` books the session-close GL correction; `billing` + `payment` are
/// driven only by offline REFUND replays (the same reversal pair `return_sale` uses).
///
/// Read routes are tenant-scoped: the same `company_auth` layer wraps them, so the request runs inside
/// `with_request_scope` (app.company_id bound on a dedicated connection). The generic list/get path
/// executes through `company_scope::fetch_*_scoped`, which rides that connection, so RLS returns only
/// the caller's company rows. Unauthenticated reads get 401 — these surfaces expose company data.
pub fn create_guarded_pos_routes(
    m: &PosModule,
    pool: PgPool,
    verifier: CompanyVerifier,
    sink: Arc<dyn PosEventSink>,
    tax: Arc<dyn PosTaxComputePort>,
    variance: Arc<dyn PosCashVariancePort>,
    billing: Arc<dyn BillingPort>,
    payment: Arc<dyn PaymentPort>,
) -> Router {
    create_guarded_pos_routes_inner(m, pool, verifier, sink, tax, variance, billing, payment, None)
}

/// Like [`create_guarded_pos_routes`] but opts the write service into outbox-backed
/// `PosTenderCompleted` staging (durable recognition). The composing service must run
/// `backbone_outbox::outbox::migrate(pool, &outbox_schema)` at startup and a relay
/// (`backbone_outbox::runner`) that drains `{outbox_schema}.outbox_events` → recognition.
pub fn create_guarded_pos_routes_with_outbox(
    m: &PosModule,
    pool: PgPool,
    verifier: CompanyVerifier,
    sink: Arc<dyn PosEventSink>,
    tax: Arc<dyn PosTaxComputePort>,
    variance: Arc<dyn PosCashVariancePort>,
    billing: Arc<dyn BillingPort>,
    payment: Arc<dyn PaymentPort>,
    outbox_schema: String,
) -> Router {
    create_guarded_pos_routes_inner(m, pool, verifier, sink, tax, variance, billing, payment, Some(outbox_schema))
}

fn create_guarded_pos_routes_inner(
    m: &PosModule,
    pool: PgPool,
    verifier: CompanyVerifier,
    sink: Arc<dyn PosEventSink>,
    tax: Arc<dyn PosTaxComputePort>,
    variance: Arc<dyn PosCashVariancePort>,
    billing: Arc<dyn BillingPort>,
    payment: Arc<dyn PaymentPort>,
    outbox_schema: Option<String>,
) -> Router {
    let mut svc = PosWriteService::with_sink(pool, sink);
    if let Some(schema) = outbox_schema {
        svc = svc.with_outbox(schema);
    }
    let write = WriteState { svc: Arc::new(svc), tax, variance, billing, payment };
    let reads = Router::new()
        .merge(create_pos_profile_read_routes(m.pos_profile_service.clone()))
        .merge(create_pos_opening_entry_read_routes(m.pos_opening_entry_service.clone()))
        .merge(create_pos_invoice_read_routes(m.pos_invoice_service.clone()))
        // Restaurant lane: floor plans + tables (seating surfaces) and the discount masters (the
        // order-level discount rate source) as tenant-scoped reads.
        .merge(create_pos_floor_plan_read_routes(m.pos_floor_plan_service.clone()))
        .merge(create_pos_table_read_routes(m.pos_table_service.clone()))
        .merge(create_pos_discount_read_routes(m.pos_discount_service.clone()))
        .route_layer(from_fn_with_state(verifier.clone(), company_auth));
    Router::new()
        .merge(reads)
        .merge(write_routes(write, verifier))
}

// --- placeholders for the priced-route-only state shape -------------------------
//
// The priced constructor mounts exactly one route; the state type carries every port so the handler
// compiles against the same shape. These implementations refuse if ever reached (they never are —
// no route mounts them), and their refusal is loud rather than silent.

struct NoVariance;
#[async_trait::async_trait]
impl PosCashVariancePort for NoVariance {
    async fn book_cash_variance(&self, _req: &crate::application::service::pos_ports::CashVarianceRequest) -> Result<crate::application::service::pos_ports::CashVarianceAck, crate::application::service::pos_ports::PosRejected> {
        Err(crate::application::service::pos_ports::PosRejected {
            code: "variance_not_mounted".into(),
            message: "this constructor mounts only the priced ring; variance booking is not mounted here".into(),
        })
    }
}
struct NoBilling;
#[async_trait::async_trait]
impl BillingPort for NoBilling {
    async fn raise_and_post(&self, _req: &crate::application::service::pos_ports::SaleInvoiceRequest) -> Result<crate::application::service::pos_ports::InvoiceAck, crate::application::service::pos_ports::PosRejected> {
        Err(crate::application::service::pos_ports::PosRejected { code: "billing_not_mounted".into(), message: "this constructor mounts only the priced ring".into() })
    }
    async fn credit_note(&self, _req: &crate::application::service::pos_ports::CreditNoteRequest) -> Result<crate::application::service::pos_ports::ReversalAck, crate::application::service::pos_ports::PosRejected> {
        Err(crate::application::service::pos_ports::PosRejected { code: "billing_not_mounted".into(), message: "this constructor mounts only the priced ring".into() })
    }
}
struct NoPayment;
#[async_trait::async_trait]
impl PaymentPort for NoPayment {
    async fn settle(&self, _req: &crate::application::service::pos_ports::SettlementRequest) -> Result<crate::application::service::pos_ports::SettlementAck, crate::application::service::pos_ports::PosRejected> {
        Err(crate::application::service::pos_ports::PosRejected { code: "payment_not_mounted".into(), message: "this constructor mounts only the priced ring".into() })
    }
    async fn refund(&self, _req: &crate::application::service::pos_ports::RefundRequest) -> Result<crate::application::service::pos_ports::ReversalAck, crate::application::service::pos_ports::PosRejected> {
        Err(crate::application::service::pos_ports::PosRejected { code: "payment_not_mounted".into(), message: "this constructor mounts only the priced ring".into() })
    }
}
