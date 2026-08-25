//! Cashier-session `open_session` tenant scoping under the production RLS fence
//! (hand-authored, user-owned).
//!
//! Why a dedicated fenced harness: the defect class these tests pin lives where the
//! row-level-security fence meets the one-open-session-per-register unique index, and
//! part of it is only observable against a real restricted app role (NOBYPASSRLS,
//! fences armed) — the posture a composed service connects with in production. The
//! harness mints, on a scratch Postgres:
//!
//! - a scratch database carrying the module's real migrations (fences + the
//!   company-scoped one-open-session unique included),
//! - a `NOBYPASSRLS` app role with only the grants a production app role holds,
//!
//! and runs `open_session` on a pool logged in as that role. The three regressions
//! pinned here:
//!
//! 1. the register slot is the TENANT's own — an open session row carrying a register
//!    uuid in a second tenant must not block the owning tenant from opening that
//!    register (the earlier unique keyed on the register uuid alone was global across
//!    tenants and did exactly that);
//! 2. a register uuid the tenant does not own — unknown outright, or another tenant's —
//!    is a typed `profile_not_found` refusal, never a session opened against nothing
//!    (and never a differently-typed answer that would leak whose register it is);
//! 3. the second open on the SAME register in the SAME tenant still refuses with the
//!    typed `session_already_open` after the unique index re-keying — the service maps
//!    the violation by constraint name, and the new index name keeps the column
//!    fragment that mapping keys on.
//!
//! Gate: `BACKBONE_POS_RLS_DSN` — a DSN whose user may create databases and roles (the
//! armed test container shape). Defaults to `127.0.0.1:5434`; the tests skip when it is
//! unreachable, so environments without the container still build and pass.

mod support;
use support::{at, d, Recorder};

use sqlx::PgPool;
use uuid::Uuid;

use backbone_orm::company_scope;
use backbone_pos::application::service::pos_events::PosEvent;
use backbone_pos::application::service::pos_write_service::{NewSession, PosError, PosWriteService};

fn rls_dsn() -> String {
    std::env::var("BACKBONE_POS_RLS_DSN")
        .unwrap_or_else(|_| "postgresql://root:password@127.0.0.1:5434/postgres".to_string())
}

/// `postgresql://user:pass@host:port/db` → `host:port`
fn host_port(dsn: &str) -> String {
    dsn.rsplit('@').next().unwrap().split('/').next().unwrap().to_string()
}

/// The same DSN pointed at a different database (assumes a plain `user:pass@host:port/db`
/// shape — what the armed test container uses).
fn with_db(dsn: &str, db: &str) -> String {
    let (before_path, _) = dsn.rsplit_once('/').expect("DSN with a database path");
    format!("{before_path}/{db}")
}

/// The scratch lab: owner pool (migrations, grants, fenced verification reads) + the
/// restricted app-role pool the service under test runs on. `teardown()` drops
/// everything it created.
struct RlsLab {
    admin: PgPool,
    app: PgPool,
    control_dsn: String,
    db: String,
    role: String,
}

impl RlsLab {
    async fn teardown(self) {
        let RlsLab { admin, app, control_dsn, db, role } = self;
        app.close().await;
        admin.close().await;
        let control = PgPool::connect(&control_dsn).await.expect("reconnect control");
        // Best-effort with bounded retries: the drops can race backends that are still
        // exiting after the pools closed. These are scratch objects on a scratch
        // container — leaving one behind on a persistent failure is acceptable noise,
        // failing the TEST over cleanup is not.
        let mut last_err = None;
        for attempt in 0..5u32 {
            let r = match sqlx::query(&format!(r#"DROP DATABASE IF EXISTS "{db}" WITH (FORCE)"#))
                .execute(&control)
                .await
            {
                Ok(_) => {
                    sqlx::query(&format!(r#"DROP ROLE IF EXISTS "{role}"#))
                        .execute(&control)
                        .await
                        .map(|_| ())
                }
                Err(e) => Err(e),
            };
            match r {
                Ok(_) => {
                    last_err = None;
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(200 * u64::from(attempt + 1))).await;
                }
            }
        }
        if let Some(e) = last_err {
            eprintln!("scratch cleanup left {db}/{role} behind: {e}");
        }
        control.close().await;
    }
}

/// Mint the lab, or `None` when the fenced container is unreachable (test skips).
async fn lab() -> Option<RlsLab> {
    let dsn = rls_dsn();
    let control = match PgPool::connect(&dsn).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping: fenced scratch Postgres unreachable at {dsn}: {e}");
            return None;
        }
    };
    let suffix = &Uuid::new_v4().simple().to_string()[..8];
    let db = format!("pos_open_probe_{suffix}");
    let role = format!("pos_open_app_{suffix}");
    let pw = "pos_open_pw";

    let made = sqlx::query(&format!(r#"CREATE DATABASE "{db}""#)).execute(&control).await;
    if let Err(e) = made {
        eprintln!("skipping: cannot create scratch database on {dsn}: {e}");
        let _ = control.close().await;
        return None;
    }
    let host = host_port(&dsn);
    let admin = PgPool::connect(&with_db(&dsn, &db)).await.expect("admin connect");
    control.close().await;

    // The module's real migrations — RLS fences and the company-scoped one-open-session
    // unique included. Resolved from the crate manifest dir at runtime (the compile-time
    // `migrate!` macro cannot take a path relative to a tests/ file).
    let migrations = sqlx::migrate::Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
    )
    .await
    .expect("locate module migrations");
    migrations.run(&admin).await.expect("module migrations");

    // The restricted app role: exactly the fence posture a production deployment runs
    // under.
    sqlx::query(&format!(
        r#"CREATE ROLE "{role}" LOGIN PASSWORD '{pw}' NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE"#
    ))
    .execute(&admin)
    .await
    .expect("create app role");
    sqlx::query(&format!(r#"GRANT USAGE ON SCHEMA pos TO "{role}""#))
        .execute(&admin)
        .await
        .expect("schema usage grant");
    sqlx::query(&format!(
        r#"GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA pos TO "{role}""#
    ))
    .execute(&admin)
    .await
    .expect("pos table grants");

    let app = PgPool::connect(&format!("postgresql://{role}:{pw}@{host}/{db}"))
        .await
        .expect("app-role connect");
    Some(RlsLab { admin, app, control_dsn: dsn, db, role })
}

/// Seed one register row for the company as the OWNER (scoped — the owner is itself
/// subject to the forced fence). The second tenant deliberately gets NO register row.
async fn seed_register(admin: &PgPool, company: Uuid) -> Uuid {
    let profile = Uuid::new_v4();
    company_scope::with_company_scope(Some(company), async {
        company_scope::execute_scoped(
            admin,
            sqlx::query(
                r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, tax_template_ids, allow_discount, status)
                   VALUES ($1,$2,'Register 1','IDR','[]'::jsonb,true,'active')"#,
            )
            .bind(profile)
            .bind(company),
        )
        .await
        .expect("seed profile");
    })
    .await;
    profile
}

fn new_session(company: Uuid, profile: Uuid) -> NewSession {
    NewSession {
        company_id: company,
        pos_profile_id: profile,
        branch_id: None,
        cashier_party_id: Uuid::new_v4(),
        opened_at: at(),
        opening_balances: vec![("cash".to_string(), d("500000"))],
    }
}

/// Count the company's open sessions on one register, read as the owner THROUGH the
/// fence (scoped) — the app role cannot see rows outside its own scope, and neither can
/// an unscoped owner under FORCE ROW LEVEL SECURITY.
async fn open_sessions_on(admin: &PgPool, company: Uuid, profile: Uuid) -> i64 {
    company_scope::with_company_scope(Some(company), async {
        company_scope::fetch_one_scalar_scoped(
            admin,
            sqlx::query_scalar(
                r#"SELECT count(*) FROM pos.pos_opening_entries
                   WHERE company_id=$1 AND pos_profile_id=$2 AND status='open'::pos_session_status
                     AND (metadata->>'deleted_at') IS NULL"#,
            )
            .bind(company)
            .bind(profile),
        )
        .await
        .expect("count open sessions")
    })
    .await
}

// ---- the tests ---------------------------------------------------------------------------------

/// The same register uuid may hold an open session in TWO tenants at once — the slot is
/// the tenant's own, not the uuid's. A scoped row naming the register in a second tenant
/// (the shape a writer that skipped tenant validation produces) must not block the
/// owning tenant's open: under the earlier uuid-global unique, exactly that block made
/// the rightful tenant's register refuse with `session_already_open`.
#[tokio::test]
async fn the_same_register_uuid_open_in_two_tenants_does_not_block_the_owner() {
    let Some(l) = lab().await else { return };
    let (company_a, company_b) = (Uuid::new_v4(), Uuid::new_v4());
    let profile = seed_register(&l.admin, company_a).await;

    // Canary — the harness is only meaningful while the fence is genuinely armed for
    // this role: an opening-entries INSERT with NO company bound must be REJECTED. The
    // day this canary passes, the tests above it have lost their power to detect the
    // defect class.
    let unbound = sqlx::query(
        r#"INSERT INTO pos.pos_opening_entries (id, company_id, pos_profile_id, cashier_party_id, opened_at, opening_balances)
           VALUES ($1,$2,$3,$4,$5,'[]'::jsonb)"#,
    )
    .bind(Uuid::new_v4())
    .bind(company_a)
    .bind(profile)
    .bind(Uuid::new_v4())
    .bind(at().and_utc())
    .execute(&l.app)
    .await;
    match unbound {
        Err(e) => assert!(
            e.to_string().to_lowercase().contains("row-level security"),
            "expected the fence to reject an unbound opening-entries INSERT, got: {e}"
        ),
        Ok(_) => panic!("fence canary failed: the app role inserted into pos_opening_entries unbound — RLS is not armed for this role, the harness proves nothing"),
    }

    // Tenant B opens "its" session on the SAME register uuid — a scoped write the fence
    // accepts (the row is B's own); only the unique index decides whether the slot is
    // occupied. The company-scoped key must admit it: B's slot is B's, regardless of
    // what the uuid names elsewhere.
    company_scope::with_company_scope(Some(company_b), async {
        company_scope::execute_scoped(
            &l.app,
            sqlx::query(
                r#"INSERT INTO pos.pos_opening_entries (id, company_id, pos_profile_id, cashier_party_id, opened_at, opening_balances)
                   VALUES ($1,$2,$3,$4,$5,'[]'::jsonb)"#,
            )
            .bind(Uuid::new_v4())
            .bind(company_b)
            .bind(profile)
            .bind(Uuid::new_v4())
            .bind(at().and_utc()),
        )
        .await
        .expect("tenant B's scoped insert on the same register uuid must be admitted");
    })
    .await;

    // The rightful tenant's open — refused by the earlier uuid-global unique with
    // session_already_open — must now succeed.
    let rec = Recorder::default();
    let w = PosWriteService::with_sink(l.app.clone(), std::sync::Arc::new(rec.clone()));
    w.open_session(new_session(company_a, profile))
        .await
        .expect("the owning tenant's open must not be blocked by another tenant's row on the same register uuid");

    // Both tenants now hold an open session on that register uuid — one row each.
    assert_eq!(open_sessions_on(&l.admin, company_a, profile).await, 1);
    assert_eq!(open_sessions_on(&l.admin, company_b, profile).await, 1);

    let opened_for_a = rec.events.lock().unwrap().iter().any(|e| matches!(
        e,
        PosEvent::PosSessionOpened(ev) if ev.company_id == company_a && ev.pos_profile_id == profile
    ));
    assert!(opened_for_a, "the owning tenant's open publishes PosSessionOpened");

    l.teardown().await;
}

/// A register uuid the tenant does not own is a typed refusal — never a session opened
/// against nothing. The uuid may be unknown outright OR belong to another tenant; the
/// fence cannot and must not tell the two apart, so both answers are the same
/// `profile_not_found` (a differently-typed answer for the foreign uuid would be a
/// cross-tenant oracle: it would reveal the register exists — and is open — elsewhere).
#[tokio::test]
async fn a_register_uuid_the_tenant_does_not_own_is_a_typed_refusal() {
    let Some(l) = lab().await else { return };
    let (company_a, company_b) = (Uuid::new_v4(), Uuid::new_v4());
    let profile = seed_register(&l.admin, company_a).await;

    let rec = Recorder::default();
    let w = PosWriteService::with_sink(l.app.clone(), std::sync::Arc::new(rec.clone()));

    // Foreign uuid — exists, but in tenant A.
    let err = w
        .open_session(new_session(company_b, profile))
        .await
        .expect_err("a register uuid owned by another tenant must refuse");
    assert!(
        matches!(err, PosError::ProfileNotFound(id) if id == profile),
        "expected profile_not_found for a foreign register uuid, got {err:?}"
    );
    assert_eq!(err.code(), "profile_not_found");
    assert_eq!(err.http_status(), 404);

    // Unknown uuid — exists nowhere. The SAME typed refusal: the caller cannot learn
    // which of the two it was.
    let unknown = Uuid::new_v4();
    let err = w
        .open_session(new_session(company_b, unknown))
        .await
        .expect_err("an unknown register uuid must refuse");
    assert!(
        matches!(err, PosError::ProfileNotFound(id) if id == unknown),
        "expected profile_not_found for an unknown register uuid, got {err:?}"
    );
    assert_eq!(err.http_status(), 404);

    // Nothing was written and nothing was announced: the tenant holds zero sessions and
    // the sink stayed silent.
    assert_eq!(open_sessions_on(&l.admin, company_b, profile).await, 0);
    assert!(rec.events.lock().unwrap().is_empty(), "a refused open publishes nothing");

    l.teardown().await;
}

/// The second open on the SAME register in the SAME tenant still refuses with the TYPED
/// `session_already_open`. The service maps the unique violation by constraint name and
/// the re-keyed index carries a new one — this test is the guard that the mapping
/// followed the re-keying (a mismatch would degrade the refusal to a raw internal error).
#[tokio::test]
async fn a_second_open_on_the_same_register_in_one_tenant_still_refuses_typed() {
    let Some(l) = lab().await else { return };
    let company = Uuid::new_v4();
    let profile = seed_register(&l.admin, company).await;

    let rec = Recorder::default();
    let w = PosWriteService::with_sink(l.app.clone(), std::sync::Arc::new(rec.clone()));

    w.open_session(new_session(company, profile))
        .await
        .expect("first open on the register");

    let err = w
        .open_session(new_session(company, profile))
        .await
        .expect_err("a second open on the same register in the same tenant must refuse");
    assert!(matches!(err, PosError::SessionAlreadyOpen), "expected session_already_open, got {err:?}");
    assert_eq!(err.code(), "session_already_open");
    assert_eq!(err.http_status(), 422);

    // Exactly one session row, exactly one open event.
    assert_eq!(open_sessions_on(&l.admin, company, profile).await, 1);
    assert_eq!(
        rec.events.lock().unwrap().iter().filter(|e| matches!(e, PosEvent::PosSessionOpened(_))).count(),
        1
    );

    l.teardown().await;
}
