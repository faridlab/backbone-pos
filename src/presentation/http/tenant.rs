//! Tenant authentication for the guarded POS surface (hand-authored, user-owned).
//!
//! The counter writers must NOT trust a client-supplied `company_id` — before this, every guarded
//! write took `company_id` off the JSON body, so a caller could stamp an invoice with any tenant and
//! (because `ring_sale` resolved the session by id alone) write into another company's session. Here we
//! derive the tenant from a **signed** Bearer access token: the middleware validates the JWT, requires a
//! `company_id` claim, and inserts a `TenantContext` into request extensions; handlers read it via the
//! `TenantContext` extractor. No `company_id` crosses the wire in a request body anymore.
//!
//! Deliberately self-contained (HS256, `jsonwebtoken` — already a POS dependency): the framework's
//! `backbone_auth::Claims` carries no tenant and no module currently shares a tenant extractor. When a
//! second guarded module needs this, promote `TenantContext`/`TenantVerifier` to the framework. The
//! composing service builds one `TenantVerifier` from `JWT_SECRET` and hands it to
//! `create_guarded_pos_routes`.

use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{header, request::Parts, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The tenant + subject proven by a validated access token. Populated by [`tenant_auth`] and read by
/// guarded handlers via the [`FromRequestParts`] impl below.
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub company_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub user_id: String,
}

/// The access-token claims the POS surface trusts. `company_id` is REQUIRED to write — a token without
/// it is rejected (401). `branch_id` is optional (org tree not yet modelled).
#[derive(Debug, Serialize, Deserialize)]
pub struct TenantClaims {
    /// Subject (the authenticated user/principal id).
    pub sub: String,
    /// Expiry (seconds since epoch) — standard JWT claim, validated.
    pub exp: usize,
    #[serde(default)]
    pub company_id: Option<Uuid>,
    #[serde(default)]
    pub branch_id: Option<Uuid>,
}

/// Verifier the composing service builds once (from `JWT_SECRET`) and clones into the guarded routes.
#[derive(Clone)]
pub struct TenantVerifier {
    key: Arc<DecodingKey>,
    validation: Arc<Validation>,
}

impl TenantVerifier {
    /// HS256 verifier over a shared secret (the common single-service deployment).
    pub fn hs256(secret: &[u8]) -> Self {
        Self {
            key: Arc::new(DecodingKey::from_secret(secret)),
            validation: Arc::new(Validation::new(Algorithm::HS256)),
        }
    }

    /// Validate a raw token → a tenant context, or `None` if the signature/expiry is bad or the
    /// `company_id` claim is absent.
    fn verify(&self, token: &str) -> Option<TenantContext> {
        let data = decode::<TenantClaims>(token, &self.key, &self.validation).ok()?;
        let c = data.claims;
        Some(TenantContext {
            company_id: c.company_id?,
            branch_id: c.branch_id,
            user_id: c.sub,
        })
    }
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "unauthorized", "message": message })),
    )
        .into_response()
}

/// Middleware: validate the Bearer token and insert a [`TenantContext`]; reject with 401 otherwise.
/// Mount on the guarded write routes via `from_fn_with_state(verifier, tenant_auth)`.
pub async fn tenant_auth(State(verifier): State<TenantVerifier>, mut req: Request, next: Next) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer ")));
    let Some(token) = token else {
        return unauthorized("missing bearer token");
    };
    match verifier.verify(token) {
        Some(ctx) => {
            req.extensions_mut().insert(ctx);
            next.run(req).await
        }
        None => unauthorized("invalid token or missing company_id claim"),
    }
}

/// Extractor: pull the [`TenantContext`] the middleware inserted (401 if the route was reached without
/// it — a wiring error, since the middleware rejects unauthenticated requests first).
#[async_trait::async_trait]
impl<S: Send + Sync> FromRequestParts<S> for TenantContext {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<TenantContext>()
            .cloned()
            .ok_or_else(|| unauthorized("unauthenticated"))
    }
}
