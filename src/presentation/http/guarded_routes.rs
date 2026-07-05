//! Guarded route composition — the RECOMMENDED way to mount the POS module.
//!
//! Hand-authored (user-owned). Read documents + **validated writes** (open session / ring sale / add
//! tender / close session); generic CRUD is NOT mounted, so a caller cannot write a ticket with
//! inconsistent totals or reopen a closed session. `recognize_sale` drives billing + payment through
//! ports (a composition layer), so it is service/job-driven, not an HTTP route.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::application::service::pos_write_service::{
    NewClose, NewSale, NewSaleLine, NewSession, PosError, PosWriteService,
};
use crate::PosModule;

use super::{
    create_pos_invoice_read_routes, create_pos_opening_entry_read_routes, create_pos_profile_read_routes,
};

#[derive(Debug, Serialize)]
struct ErrorBody { error: String, message: String }
#[derive(Debug, Serialize)]
struct IdResponse { id: Uuid }
fn err(e: PosError) -> axum::response::Response {
    let s = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (s, Json(ErrorBody { error: e.code(), message: e.to_string() })).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpeningBalanceBody { method: String, amount: Decimal }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenSessionBody {
    company_id: Uuid,
    pos_profile_id: Uuid,
    #[serde(default)] branch_id: Option<Uuid>,
    cashier_party_id: Uuid,
    opened_at: chrono::NaiveDateTime,
    #[serde(default)] opening_balances: Vec<OpeningBalanceBody>,
}
async fn open_session(State(svc): State<Arc<PosWriteService>>, Json(b): Json<OpenSessionBody>) -> axum::response::Response {
    let s = NewSession {
        company_id: b.company_id, pos_profile_id: b.pos_profile_id, branch_id: b.branch_id,
        cashier_party_id: b.cashier_party_id, opened_at: b.opened_at,
        opening_balances: b.opening_balances.into_iter().map(|o| (o.method, o.amount)).collect(),
    };
    match svc.open_session(s).await {
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
    quantity: Decimal,
    unit_price: Decimal,
    #[serde(default)] discount_amount: Decimal,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RingSaleBody {
    company_id: Uuid,
    pos_profile_id: Uuid,
    opening_entry_id: Uuid,
    #[serde(default)] branch_id: Option<Uuid>,
    #[serde(default)] customer_id: Option<Uuid>,
    receipt_number: String,
    posting_at: chrono::NaiveDateTime,
    lines: Vec<SaleLineBody>,
    #[serde(default)] tax_total: Decimal,
    #[serde(default)] round_to: Option<Decimal>,
}
async fn ring_sale(State(svc): State<Arc<PosWriteService>>, Json(b): Json<RingSaleBody>) -> axum::response::Response {
    let sale = NewSale {
        company_id: b.company_id, pos_profile_id: b.pos_profile_id, opening_entry_id: b.opening_entry_id,
        branch_id: b.branch_id, customer_id: b.customer_id, receipt_number: b.receipt_number, posting_at: b.posting_at,
        lines: b.lines.into_iter().map(|l| NewSaleLine {
            item_id: l.item_id, revenue_account_id: l.revenue_account_id, description: l.description,
            quantity: l.quantity, unit_price: l.unit_price, discount_amount: l.discount_amount,
        }).collect(),
        tax_total: b.tax_total, round_to: b.round_to,
    };
    match svc.ring_sale(sale).await {
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
async fn add_tender(State(svc): State<Arc<PosWriteService>>, Json(b): Json<TenderBody>) -> axum::response::Response {
    match svc.add_tender(b.pos_invoice_id, &b.payment_method, b.amount, b.reference_no).await {
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
struct CloseBody {
    company_id: Uuid,
    opening_entry_id: Uuid,
    cashier_party_id: Uuid,
    closed_at: chrono::NaiveDateTime,
    #[serde(default)] counted: Vec<CountedBody>,
}
async fn close_session(State(svc): State<Arc<PosWriteService>>, Json(b): Json<CloseBody>) -> axum::response::Response {
    let c = NewClose {
        company_id: b.company_id, opening_entry_id: b.opening_entry_id, cashier_party_id: b.cashier_party_id,
        closed_at: b.closed_at, counted: b.counted.into_iter().map(|x| (x.method, x.amount)).collect(),
    };
    match svc.close_session(c).await {
        Ok(o) => (StatusCode::OK, Json(serde_json::json!({
            "closingId": o.closing_id, "differenceTotal": o.difference_total,
        }))).into_response(),
        Err(e) => err(e),
    }
}

fn write_routes(svc: Arc<PosWriteService>) -> Router {
    Router::new()
        .route("/pos-sessions", post(open_session))
        .route("/pos-sales", post(ring_sale))
        .route("/pos-tenders", post(add_tender))
        .route("/pos-sessions/close", post(close_session))
        .with_state(svc)
}

/// Mount the POS module: read documents + validated writes. Generic mutation is not mounted; sale
/// recognition (billing + payment handoff) is service/job-driven via the ports.
/// **Prefer this over `PosModule::all_crud_routes()` for any real deployment.**
pub fn create_guarded_pos_routes(m: &PosModule, pool: PgPool) -> Router {
    let write = Arc::new(PosWriteService::new(pool));
    Router::new()
        .merge(create_pos_profile_read_routes(m.pos_profile_service.clone()))
        .merge(create_pos_opening_entry_read_routes(m.pos_opening_entry_service.clone()))
        .merge(create_pos_invoice_read_routes(m.pos_invoice_service.clone()))
        .merge(write_routes(write))
}
