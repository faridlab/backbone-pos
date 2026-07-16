//! Tenancy guard for the POS write surface (hand-authored, user-owned).
//!
//! Proves the fix for the maturity-council cross-tenant finding: the guarded writers derive `company_id`
//! from a signed Bearer token, not a request body, and `ring_sale` scopes the session lookup by it. Runs
//! the real router in-process via `tower::ServiceExt::oneshot`. Requires DATABASE_URL with the pos schema.
//!
//! TG-1  unauthenticated write            → 401
//! TG-2  token with no company_id claim    → 401
//! TG-3  token for company B vs A's session → rejected (cross-tenant write closed)
//! TG-4  token for company A vs A's session → 201 Created

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use backbone_pos::application::service::pos_cart_pricing::{
    CartPriceRequest, CartPricingError, CartPricingPort, PricedCart, PricedCartLine,
};
use backbone_pos::application::service::pos_events::LoggingSink;
use backbone_pos::application::service::pos_write_service::{NewSession, PosWriteService};
use backbone_pos::presentation::http::{
    create_guarded_pos_priced_route, create_guarded_pos_routes, TenantVerifier,
};
use backbone_pos::PosModule;
use std::sync::Arc;

const SECRET: &[u8] = b"tenant-guard-test-secret";

#[derive(Serialize)]
struct TestClaims {
    sub: String,
    exp: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    company_id: Option<Uuid>,
}

/// Mint an HS256 token. `company_id = None` models a token that authenticates a user but carries no
/// tenant — it must not be allowed to write.
fn token(company_id: Option<Uuid>) -> String {
    let claims = TestClaims { sub: "cashier-1".into(), exp: 9_999_999_999, company_id };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(SECRET)).unwrap()
}

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5433/backbone_pos".to_string());
    PgPool::connect(&url).await.expect("connect DB")
}

fn at() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(2026, 7, 14).unwrap().and_hms_opt(9, 0, 0).unwrap()
}

fn ring_body(opening_entry_id: Uuid, profile: Uuid) -> String {
    // NOTE: no `companyId` field — it is derived from the token now.
    json!({
        "posProfileId": profile,
        "openingEntryId": opening_entry_id,
        "receiptNumber": format!("R-{}", &Uuid::new_v4().simple().to_string()[..8]),
        "postingAt": "2026-07-14T09:00:00",
        "lines": [{ "itemId": Uuid::new_v4(), "quantity": "1", "unitPrice": "100000" }],
    })
    .to_string()
}

fn post(uri: &str, body: String, bearer: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method("POST").uri(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b.body(Body::from(body)).unwrap()
}

/// Seed a non-PKP register (tax_rate 0) — ring_sale now loads the profile to compute server-side PPN.
async fn seed_profile(pool: &sqlx::PgPool, id: Uuid, company: Uuid) {
    sqlx::query("INSERT INTO pos.pos_profiles (id, company_id, name, currency, allow_discount, is_active) VALUES ($1,$2,'Register 1','IDR',true,true)")
        .bind(id).bind(company).execute(pool).await.unwrap();
}

#[tokio::test]
async fn pos_write_surface_enforces_tenant_from_principal() {
    let pool = pool().await;
    let company_a = Uuid::new_v4();
    let company_b = Uuid::new_v4();
    let profile = Uuid::new_v4();
    seed_profile(&pool, profile, company_a).await;

    // Seed an OPEN session owned by company A (real write path, not raw SQL).
    let svc = PosWriteService::new(pool.clone());
    let session_a = svc
        .open_session(NewSession {
            company_id: company_a,
            pos_profile_id: profile,
            branch_id: None,
            cashier_party_id: Uuid::new_v4(),
            opened_at: at(),
            opening_balances: vec![],
        })
        .await
        .expect("seed open session for company A");

    let module = PosModule::builder().with_database(pool.clone()).build().expect("build PosModule");
    let app = create_guarded_pos_routes(&module, pool.clone(), TenantVerifier::hs256(SECRET), Arc::new(LoggingSink));

    // TG-1: no token → 401.
    let r = app.clone().oneshot(post("/pos-sales", ring_body(session_a, profile), None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "TG-1: unauthenticated write must be 401");

    // TG-2: token without a company_id claim → 401.
    let r = app
        .clone()
        .oneshot(post("/pos-sales", ring_body(session_a, profile), Some(&token(None))))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "TG-2: token lacking company_id must be 401");

    // TG-3: company B's token against company A's session → rejected (not Created). The cross-tenant
    // write the council flagged: previously the body carried company_id and ring_sale found the session
    // by id alone. Now the session lookup is scoped by the token's company_id, so B cannot see A's.
    let r = app
        .clone()
        .oneshot(post("/pos-sales", ring_body(session_a, profile), Some(&token(Some(company_b)))))
        .await
        .unwrap();
    assert_ne!(r.status(), StatusCode::CREATED, "TG-3: cross-tenant ring must NOT create an invoice");
    assert!(r.status().is_client_error(), "TG-3: cross-tenant ring is a client error, got {}", r.status());

    // TG-4: company A's own token against its session → 201 Created.
    let r = app
        .clone()
        .oneshot(post("/pos-sales", ring_body(session_a, profile), Some(&token(Some(company_a)))))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED, "TG-4: authenticated same-tenant ring must succeed");
}

/// A stand-in pricer: 50% off every line. Proves the priced route rings from the PRICER's result, not
/// the client's listPrice (the real service wires promo's resolve_cart here).
struct HalfOffPricer;
#[async_trait::async_trait]
impl CartPricingPort for HalfOffPricer {
    async fn price_cart(&self, req: &CartPriceRequest) -> Result<PricedCart, CartPricingError> {
        let lines: Vec<PricedCartLine> = req
            .lines
            .iter()
            .map(|l| {
                let unit = l.list_price / rust_decimal::Decimal::from(2);
                PricedCartLine { line_ref: l.line_ref, unit_price: unit, net_line_total: unit * l.quantity }
            })
            .collect();
        let total = lines.iter().map(|l| l.net_line_total).sum();
        Ok(PricedCart { lines, reward_lines: vec![], total })
    }
}

#[tokio::test]
async fn priced_route_rings_from_the_server_pricer_not_the_client_price() {
    let pool = pool().await;
    let company_a = Uuid::new_v4();
    let profile = Uuid::new_v4();
    seed_profile(&pool, profile, company_a).await;

    let svc = PosWriteService::new(pool.clone());
    let session_a = svc
        .open_session(NewSession {
            company_id: company_a,
            pos_profile_id: profile,
            branch_id: None,
            cashier_party_id: Uuid::new_v4(),
            opened_at: at(),
            opening_balances: vec![],
        })
        .await
        .expect("seed open session");

    let app = create_guarded_pos_priced_route(pool.clone(), TenantVerifier::hs256(SECRET), Arc::new(HalfOffPricer), Arc::new(LoggingSink));
    let receipt = format!("RP-{}", &Uuid::new_v4().simple().to_string()[..8]);
    let body = json!({
        "posProfileId": profile,
        "openingEntryId": session_a,
        "receiptNumber": receipt,
        "postingAt": "2026-07-14T09:00:00",
        "lines": [{ "itemId": Uuid::new_v4(), "listPrice": "100000", "quantity": "1" }],
    })
    .to_string();

    // Unauthenticated → 401 (auth applies to the priced route too).
    let r = app.clone().oneshot(post("/pos-sales/priced", body.clone(), None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "priced route must require auth");

    // Authenticated → 201, and the ticket nets 50,000 (pricer-resolved), NOT the 100,000 listPrice.
    let r = app.oneshot(post("/pos-sales/priced", body, Some(&token(Some(company_a))))).await.unwrap();
    assert_eq!(r.status(), StatusCode::CREATED, "priced ring must succeed");

    let net: rust_decimal::Decimal =
        sqlx::query_scalar("SELECT net_total FROM pos.pos_invoices WHERE receipt_number=$1")
            .bind(&receipt)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(net.to_string(), "50000.00", "ticket net must reflect the server pricer (50% off), not the client listPrice");
}
