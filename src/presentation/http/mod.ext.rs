// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

pub mod guarded_routes;
pub use guarded_routes::{create_guarded_pos_priced_route, create_guarded_pos_priced_route_with_outbox, create_guarded_pos_routes, create_guarded_pos_routes_with_outbox, PosCredentialAdmin};
// The company guard now lives in the framework (`backbone_auth::company`, feature `axum`) — POS proved
// the pattern, and it was promoted once a second guarded module needed it. Re-exported here so the
// composing service and the TG-* tests keep importing it from this module.
pub use backbone_auth::company::{company_auth, CompanyClaims, CompanyContext, CompanyVerifier};
// Deprecated Tenant* aliases: a composing service pinned to `main` (e.g. serpa-posman-service)
// references `TenantVerifier` etc. through this module. Keep it compiling on sync; migrate to the
// Company* names (ADR-0005) and then drop these.
#[allow(deprecated)]
pub use backbone_auth::{TenantClaims, TenantContext, TenantVerifier, tenant_auth};
