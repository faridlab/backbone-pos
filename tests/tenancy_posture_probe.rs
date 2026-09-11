//! Tenancy posture probe (ADR-0029).
//!
//! The module ships NO tenancy: no tenant column, no tenant predicate in any statement,
//! and no RLS policy of its own. What it ships instead is the half-fence the composing
//! service's tenancy decorator completes: every pos base table carries ENABLE + FORCE
//! ROW LEVEL SECURITY with zero policies. This probe pins that posture from below, the
//! family pattern (proven on backbone-accounting, backbone-billing, then backbone-selling):
//!
//! - the flags are armed and the policy set is empty (schema pin);
//! - a plain non-superuser, NOBYPASSRLS role is default-DENIED — zero rows, writes
//!   refused — no matter what legacy variable is set (no policy reads `app.company_id`
//!   anymore; the decorator's org-scoped policies will, once composed);
//! - the owner/superuser pool sees its own seeded rows plainly, and the credential
//!   read completes under the AMBIENT org scope — the per-request binding a composing
//!   service resolves — while the same binding on a RESTRICTED pool returns exactly
//!   what the (absent) policies admit: nothing, until the decorator composes.
//!
//! The durable-event outbox the module writes lives in the COMPOSING service's schema,
//! not here, so the pin covers the eleven pos tables only.
//!
//! Requires DATABASE_URL (:5433/backbone_pos) reachable as a superuser (to mint and
//! tear down the probe role).

use sqlx::{PgPool, Row};
use uuid::Uuid;

use backbone_pos::infrastructure::persistence::PosManagerPinRepository;

const ROLE: &str = "pos_tenancy_probe";
const PWD: &str = "probe";

/// Role/catalog DDL serializes — two tests minting roles concurrently hit
/// "tuple concurrently updated" in the system catalogs.
static ROLE_DDL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn admin() -> PgPool {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgresql://postgres:postgres@localhost:5433/backbone_pos".to_string()
    });
    PgPool::connect(&url).await.expect("connect admin")
}

/// Shed the role's grants, then drop it. Leftover grants (from a run whose teardown never
/// reached the drop, or whose drop was swallowed) make plain DROP ROLE fail with 2BP01 —
/// DROP OWNED BY first keeps both bootstrap and teardown idempotent across runs.
async fn drop_role(admin: &PgPool) {
    let _ = sqlx::query(&format!("DROP OWNED BY {ROLE}"))
        .execute(admin)
        .await;
    let _ = sqlx::query(&format!("DROP ROLE IF EXISTS {ROLE}"))
        .execute(admin)
        .await;
}

async fn bootstrap_role(admin: &PgPool, tables: &[&str]) {
    drop_role(admin).await;
    for stmt in [
        format!("CREATE ROLE {ROLE} LOGIN PASSWORD '{PWD}' NOSUPERUSER NOBYPASSRLS"),
        format!("GRANT USAGE ON SCHEMA pos TO {ROLE}"),
    ]
    .into_iter()
    .chain(tables.iter().map(|t| {
        format!("GRANT SELECT, INSERT, UPDATE ON TABLE pos.{t} TO {ROLE}")
    })) {
        sqlx::query(&stmt).execute(admin).await.unwrap();
    }
}

// ── The schema pin: armed flags, empty policy set ─────────────────────────────

/// Every pos base table carries ENABLE + FORCE ROW LEVEL SECURITY and the module
/// ships ZERO policies — the decorator's half-fence. If a strip or regen ever drops
/// the flags, an undecorated deployment would silently become readable by any role
/// the host grants; if a policy ever reappears module-side, the decorator's
/// org-scoped policies would fight it.
#[tokio::test]
async fn tables_carry_rls_flags_and_the_module_ships_no_policy() {
    let admin = admin().await;
    let armed: Vec<String> = sqlx::query(
        "SELECT c.relname FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'pos' AND c.relkind = 'r' \
           AND c.relrowsecurity AND c.relforcerowsecurity \
         ORDER BY c.relname",
    )
    .fetch_all(&admin)
    .await
    .unwrap()
    .iter()
    .map(|r| r.get::<String, _>("relname"))
    .collect();
    for table in [
        "pos_cash_movements",
        "pos_closing_entries",
        "pos_discounts",
        "pos_floor_plans",
        "pos_invoice_items",
        "pos_invoices",
        "pos_manager_pins",
        "pos_opening_entries",
        "pos_payments",
        "pos_profiles",
        "pos_tables",
    ] {
        assert!(
            armed.iter().any(|t| t == table),
            "{table} must carry ENABLE + FORCE ROW LEVEL SECURITY"
        );
    }

    let policies: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_policy WHERE polrelid::regnamespace::text = 'pos'",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        policies, 0,
        "the module ships no RLS policy — isolation belongs to the composing service's decorator"
    );
}

// ── Default-deny until composed: the plain probe role ─────────────────────────

/// A plain non-superuser, NOBYPASSRLS role with bare grants sees NOTHING and cannot
/// write — with or without the legacy company variable set. No policy admits it (there
/// are none), and none reads `app.company_id` anymore. The owner pool still sees its
/// seeded row: the denial is the missing policy, not an empty database.
#[tokio::test]
async fn plain_role_is_default_denied_until_the_decorator_composes() {
    let _ddl = ROLE_DDL_LOCK.lock().await;
    let admin = admin().await;
    bootstrap_role(&admin, &["pos_manager_pins"]).await;

    // The owner seeds a credential row as the superuser (whom RLS can never bind). No
    // tenant column exists to set — a credential is just a row (ADR-0029).
    let party = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pos.pos_manager_pins (id, employee_party_id, pin_hash) \
         VALUES ($1, $2, 'probe')",
    )
    .bind(Uuid::new_v4())
    .bind(party)
    .execute(&admin)
    .await
    .unwrap();

    let restricted = PgPool::connect(&format!(
        "postgresql://{ROLE}:{PWD}@localhost:5433/backbone_pos"
    ))
    .await
    .expect("connect probe role");

    // Bare read: zero rows — default-deny with no policy admitting the role.
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pos.pos_manager_pins WHERE employee_party_id=$1",
    )
    .bind(party)
    .fetch_one(&restricted)
    .await
    .unwrap();
    assert_eq!(n, 0, "a role no policy admits sees zero rows");

    // The legacy company variable resurrects nothing: no policy reads it anymore
    // (the decorator's org-scoped policies will, once composed).
    let mut tx = restricted.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.company_id', $1, true)")
        .bind(Uuid::new_v4().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pos.pos_manager_pins WHERE employee_party_id=$1",
    )
    .bind(party)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(n, 0, "the legacy variable must not bypass the absent policy set");
    tx.rollback().await.unwrap();

    // A write is refused outright (no WITH CHECK policy admits the new row).
    let err = sqlx::query(
        "INSERT INTO pos.pos_manager_pins (id, employee_party_id, pin_hash) \
         VALUES ($1, $2, 'probe write')",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&restricted)
    .await;
    assert!(err.is_err(), "a write with no admitting policy must be refused");

    // The owner pool still sees its row.
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pos.pos_manager_pins WHERE employee_party_id=$1",
    )
    .bind(party)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(n, 1, "the owner pool must still see the seeded row");

    drop_role(&admin).await;
}

// ── The module-side half: the ambient org scope drives the reads ──────────────

/// Run `f` with an ambient org scope bound — the single-company emulation of what a
/// composing service resolves and binds per request.
async fn scoped<F, R>(pool: &PgPool, company: Uuid, f: F) -> R
where
    F: std::future::Future<Output = R>,
{
    backbone_orm::org_scope::with_org_request_scope(
        pool,
        backbone_orm::org_scope::OrgScope::for_company_unit(company),
        f,
    )
    .await
    .unwrap()
}

/// The ambient org scope is what the module's reads ride: with a scope bound (the
/// composed shape), the credential read completes on the request-dedicated connection
/// and the scope is visible to module code; without one, nothing is bound. Row
/// isolation itself is the decorator's — this pins the module-side binding contract
/// only.
#[tokio::test]
async fn ambient_org_scope_drives_module_reads() {
    let admin = admin().await;
    let company = Uuid::new_v4();
    let party = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pos.pos_manager_pins (id, employee_party_id, pin_hash) \
         VALUES ($1, $2, 'probe')",
    )
    .bind(Uuid::new_v4())
    .bind(party)
    .execute(&admin)
    .await
    .unwrap();
    let pins = PosManagerPinRepository::new(admin.clone());

    // Inside the scope: bound, visible to module code, the read completes and the
    // owner's row is there (the superuser is never bound by RLS).
    let (legacy_inside, cred) = scoped(&admin, company, async {
        let scope = backbone_orm::org_scope::current_org_scope()
            .expect("the ambient scope must be bound inside");
        let cred = pins
            .fetch_credential(&admin, party)
            .await
            .expect("read completes under the ambient scope");
        (scope.legacy_company_id(), cred)
    })
    .await;
    assert_eq!(legacy_inside, Some(company));
    assert!(
        cred.is_some(),
        "the owner's read must see its own seeded credential"
    );

    // Outside: nothing is bound.
    assert!(
        backbone_orm::org_scope::current_org_scope().is_none(),
        "no ambient scope may leak past the wrapped future"
    );
}

/// The relay shape a decorated host runs: the app role (restricted, NOBYPASSRLS) with
/// the ambient scope bound per request. The binding is per-transaction — a plain pooled
/// connection cannot lose it — and the read completes, returning exactly what the
/// (still absent) policies admit: nothing, until the decorator composes.
#[tokio::test]
async fn restricted_pool_with_ambient_scope_completes_default_denied() {
    let _ddl = ROLE_DDL_LOCK.lock().await;
    let admin = admin().await;
    bootstrap_role(&admin, &["pos_manager_pins"]).await;
    let restricted = PgPool::connect(&format!(
        "postgresql://{ROLE}:{PWD}@localhost:5433/backbone_pos"
    ))
    .await
    .expect("connect probe role");

    let company = Uuid::new_v4();
    let pins = PosManagerPinRepository::new(restricted.clone());
    let cred = scoped(&restricted, company, pins.fetch_credential(&restricted, Uuid::new_v4()))
        .await
        .expect("read completes under the ambient scope");
    assert!(
        cred.is_none(),
        "the restricted role stays default-denied until the decorator installs policies"
    );
    assert!(
        backbone_orm::org_scope::current_org_scope().is_none(),
        "no ambient scope may leak past the wrapped future"
    );

    drop_role(&admin).await;
}
