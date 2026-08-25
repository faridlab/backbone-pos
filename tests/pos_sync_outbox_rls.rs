//! Durable `PosTenderCompleted` staging on the offline-sync lane, under the production RLS fence
//! (hand-authored, user-owned).
//!
//! Why a dedicated fenced harness: the defect class these tests pin — an outbox staging write that
//! runs on a transaction with no company bound — is INVISIBLE on an owner/superuser test DSN,
//! because BYPASSRLS makes even a fence-violating INSERT succeed. It only reproduces against a
//! real restricted app role (NOBYPASSRLS, fences armed), which is exactly how a composed service
//! connects in production. The harness therefore mints, on a scratch Postgres:
//!
//! - a scratch database carrying the module's real migrations (fences included),
//! - an outbox schema created by `backbone_outbox::outbox::migrate` (RLS-fenced by `multi_tenant`),
//! - a `NOBYPASSRLS` app role with only the grants a production app role holds,
//!
//! runs `sync_from_ui` on a pool logged in as that role, and reads the staged rows back through
//! the fence as the owner (scoped, since the owner is itself subject to `FORCE ROW LEVEL SECURITY`).
//!
//! Gate: `BACKBONE_POS_RLS_DSN` — a DSN whose user may create databases and roles (the armed test
//! container shape). Defaults to `127.0.0.1:5434`; the tests skip when it is unreachable, so
//! environments without the container still build and pass.

mod support;
use support::{at, d, Recorder, StubBilling, StubPayment, TestTax};

use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_orm::company_scope;
use backbone_outbox::outbox;
use backbone_pos::application::service::pos_events::PosEvent;
use backbone_pos::application::service::pos_write_service::{
    NewSyncSale, PosError, PosWriteService, SyncAction, SyncSaleLine, SyncTender,
};

/// The outbox schema the service under test stages into (granted to the app role).
const OUTBOX_SCHEMA: &str = "pos_outbox";
/// An outbox schema whose table exists but is NOT granted to the app role — staging into it fails.
const OUTBOX_SCHEMA_DENIED: &str = "pos_outbox_denied";

fn rls_dsn() -> String {
    std::env::var("BACKBONE_POS_RLS_DSN")
        .unwrap_or_else(|_| "postgresql://root:password@127.0.0.1:5434/postgres".to_string())
}

/// `postgresql://user:pass@host:port/db` → `host:port`
fn host_port(dsn: &str) -> String {
    dsn.rsplit('@').next().unwrap().split('/').next().unwrap().to_string()
}

/// The same DSN pointed at a different database (assumes a plain `user:pass@host:port/db` shape —
/// what the armed test container uses).
fn with_db(dsn: &str, db: &str) -> String {
    let (before_path, _) = dsn.rsplit_once('/').expect("DSN with a database path");
    format!("{before_path}/{db}")
}

/// The scratch lab: owner pool (migrations, grants, fenced verification reads) + the restricted
/// app-role pool the service under test runs on. `teardown()` drops everything it created.
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
        // Best-effort with bounded retries: the drops can race backends that are still exiting
        // after the pools closed. These are scratch objects on a scratch container — leaving one
        // behind on a persistent failure is acceptable noise, failing the TEST over cleanup is not.
        let mut last_err = None;
        for attempt in 0..5u32 {
            let r = match sqlx::query(&format!(r#"DROP DATABASE IF EXISTS "{db}" WITH (FORCE)"#))
                .execute(&control)
                .await
            {
                Ok(_) => {
                    sqlx::query(&format!(r#"DROP ROLE IF EXISTS "{role}""#))
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
    let db = format!("pos_rls_probe_{suffix}");
    let role = format!("pos_rls_app_{suffix}");
    let pw = "pos_rls_pw";

    let made = sqlx::query(&format!(r#"CREATE DATABASE "{db}""#)).execute(&control).await;
    if let Err(e) = made {
        eprintln!("skipping: cannot create scratch database on {dsn}: {e}");
        let _ = control.close().await;
        return None;
    }
    let host = host_port(&dsn);
    let admin = PgPool::connect(&with_db(&dsn, &db)).await.expect("admin connect");
    control.close().await;

    // The module's real migrations — RLS fences included. Resolved from the crate manifest dir at
    // runtime (the compile-time `migrate!` macro cannot take a path relative to a tests/ file).
    let migrations = sqlx::migrate::Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
    )
    .await
    .expect("locate module migrations");
    migrations.run(&admin).await.expect("module migrations");
    // Two outbox schemas: one the app role may write, one it may not.
    outbox::migrate(&admin, OUTBOX_SCHEMA).await.expect("outbox migrate");
    outbox::migrate(&admin, OUTBOX_SCHEMA_DENIED).await.expect("outbox migrate (denied)");

    // The restricted app role: exactly the fence posture a production deployment runs under.
    sqlx::query(&format!(
        r#"CREATE ROLE "{role}" LOGIN PASSWORD '{pw}' NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE"#
    ))
    .execute(&admin)
    .await
    .expect("create app role");
    sqlx::query(&format!(
        r#"GRANT USAGE ON SCHEMA pos, {OUTBOX_SCHEMA}, {OUTBOX_SCHEMA_DENIED} TO "{role}""#
    ))
    .execute(&admin)
    .await
    .expect("schema usage grants");
    sqlx::query(&format!(
        r#"GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA pos TO "{role}""#
    ))
    .execute(&admin)
    .await
    .expect("pos table grants");
    sqlx::query(&format!(
        r#"GRANT SELECT, INSERT ON ALL TABLES IN SCHEMA {OUTBOX_SCHEMA} TO "{role}""#
    ))
    .execute(&admin)
    .await
    .expect("outbox grants");
    // Deliberately NO table grant on OUTBOX_SCHEMA_DENIED: staging into it must fail.

    let app = PgPool::connect(&format!("postgresql://{role}:{pw}@{host}/{db}"))
        .await
        .expect("app-role connect");
    Some(RlsLab { admin, app, control_dsn: dsn, db, role })
}

/// Seed one register + one open session as the OWNER (scoped — the owner is itself subject to the
/// forced fence). Returns (profile, session, tax-port) for a zero-rated register.
async fn seed_register(admin: &PgPool, company: Uuid) -> (Uuid, Uuid, TestTax) {
    let template = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let session = Uuid::new_v4();
    company_scope::with_company_scope(Some(company), async {
        company_scope::execute_scoped(
            admin,
            sqlx::query(
                r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, tax_template_ids, allow_discount, status)
                   VALUES ($1,$2,'Register 1','IDR',$3,true,'active')"#,
            )
            .bind(profile)
            .bind(company)
            .bind(serde_json::json!([template.to_string()])),
        )
        .await
        .expect("seed profile");
        company_scope::execute_scoped(
            admin,
            sqlx::query(
                r#"INSERT INTO pos.pos_opening_entries (id, company_id, pos_profile_id, cashier_party_id, opened_at, opening_balances)
                   VALUES ($1,$2,$3,$4,$5,'[]'::jsonb)"#,
            )
            .bind(session)
            .bind(company)
            .bind(profile)
            .bind(Uuid::new_v4())
            .bind(at().and_utc()),
        )
        .await
        .expect("seed session");
    })
    .await;
    (profile, session, TestTax::with_rate(template, "0"))
}

/// Count the tenant's staged `PosTenderCompleted` rows for one ticket, read as the owner THROUGH
/// the fence (scoped) — the app role cannot see rows outside its own scope, and neither can an
/// unscoped owner under FORCE ROW LEVEL SECURITY.
async fn staged_tender_events(admin: &PgPool, schema: &str, company: Uuid, ticket: Uuid) -> i64 {
    company_scope::with_company_scope(Some(company), async {
        company_scope::fetch_one_scalar_scoped(
            admin,
            sqlx::query_scalar(&format!(
                r#"SELECT count(*) FROM {schema}.outbox_events
                   WHERE event_type='PosTenderCompleted' AND aggregate_id=$1 AND company_id=$2"#
            ))
            .bind(ticket.to_string())
            .bind(company),
        )
        .await
        .expect("count staged events")
    })
    .await
}

// ---- payload builders (same shape as the offline-replay behavior tests) ------------------------

fn sync_line(client: Uuid, item: Uuid, qty: &str, price: &str) -> SyncSaleLine {
    SyncSaleLine {
        client_uuid: client, item_id: item, revenue_account_id: None, description: None,
        course: None, quantity: d(qty), unit_price: d(price), discount_amount: Decimal::ZERO,
    }
}
fn tender(client: Uuid, method: &str, amount: &str) -> SyncTender {
    SyncTender { client_uuid: client, method: method.into(), amount: d(amount), reference_no: None }
}
fn sync(company: Uuid, client: Uuid, prof: Uuid, session: Uuid, lines: Vec<SyncSaleLine>, tenders: Vec<SyncTender>) -> NewSyncSale {
    NewSyncSale {
        company_id: company, client_uuid: client, pos_profile_id: prof, opening_entry_id: session,
        rescue_opening_entry_id: None, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None, posting_at: at(),
        lines, tenders, refund_of_client_uuid: None, manager: None, source_ip: None,
    }
}

// ---- the tests ---------------------------------------------------------------------------------

/// A fully-tendered replay, synced under a restricted app role, must stage the durable
/// `PosTenderCompleted` event — on BOTH replay halves (create, and update of the draft it names).
/// Under an owner/superuser DSN a staging write with no company bound still succeeds, which is why
/// this test can only exist against a NOBYPASSRLS role with the fences armed.
#[tokio::test]
async fn synced_fully_tendered_ticket_stages_the_durable_event_under_the_app_role() {
    let Some(l) = lab().await else { return };
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, session, tax) = seed_register(&l.admin, company).await;

    // Canary — the harness is only meaningful while the fence is genuinely armed for this role:
    // an outbox INSERT with NO company bound must be REJECTED. The day this canary passes, the
    // test above it has lost its power to detect the defect class.
    let unbound = sqlx::query(&format!(
        r#"INSERT INTO {OUTBOX_SCHEMA}.outbox_events
             (id, event_type, aggregate_type, aggregate_id, company_id, payload, occurred_at)
           VALUES ($1,'canary','canary','canary',$2,'{{}}'::jsonb, now())"#
    ))
    .bind(Uuid::new_v4())
    .bind(company)
    .execute(&l.app)
    .await;
    match unbound {
        Err(e) => assert!(
            e.to_string().to_lowercase().contains("row-level security"),
            "expected the fence to reject an unbound outbox INSERT, got: {e}"
        ),
        Ok(_) => panic!("fence canary failed: the app role inserted into the outbox unbound — RLS is not armed for this role, the harness proves nothing"),
    }

    let rec = Recorder::default();
    let w = PosWriteService::with_sink(l.app.clone(), std::sync::Arc::new(rec.clone()))
        .with_outbox(OUTBOX_SCHEMA);

    // Replay 1 — CREATE: one line at 50,000, fully paid in cash.
    let client = Uuid::new_v4();
    let first = w
        .sync_from_ui(
            sync(company, client, prof, session,
                vec![sync_line(Uuid::new_v4(), item, "1", "50000")],
                vec![tender(Uuid::new_v4(), "cash", "50000")]),
            &tax, &StubBilling::default(), &StubPayment,
        )
        .await
        .expect("sync under the app role");
    assert_eq!(first.action, SyncAction::Created);
    assert_eq!(first.totals.paid_total, d("50000.00"));

    // The durable event IS staged — the regression this file exists for. The pre-fix code staged
    // on an unbound second transaction, the fence rejected it, the failure was swallowed, and the
    // synced ticket stayed draft forever with zero trace.
    assert_eq!(
        staged_tender_events(&l.admin, OUTBOX_SCHEMA, company, first.pos_invoice_id).await,
        1,
        "a fully-tendered replay must stage its durable PosTenderCompleted event under a restricted app role"
    );

    // Replay 2 — UPDATE of the same draft (same client uuid, still fully tendered): the update
    // half must stage too (each fully-tendered replay stages one event; double delivery is safe —
    // recognition is replay-safe).
    let second = w
        .sync_from_ui(
            sync(company, client, prof, session,
                vec![sync_line(Uuid::new_v4(), item, "2", "50000")],
                vec![tender(Uuid::new_v4(), "card", "100000")]),
            &tax, &StubBilling::default(), &StubPayment,
        )
        .await
        .expect("replay update under the app role");
    assert_eq!(second.action, SyncAction::Updated);
    assert_eq!(second.pos_invoice_id, first.pos_invoice_id);
    assert_eq!(
        staged_tender_events(&l.admin, OUTBOX_SCHEMA, company, first.pos_invoice_id).await,
        2,
        "the update half of a fully-tendered replay must stage its durable event too"
    );

    // The ticket itself landed fully tendered (still draft until the relay recognizes it), and the
    // in-process sink fired alongside the durable staging.
    let (status, paid): (String, Decimal) = company_scope::with_company_scope(Some(company), async {
        company_scope::fetch_one_scoped(
            &l.admin,
            sqlx::query_as::<_, (String, Decimal)>(
                "SELECT status::text, paid_total FROM pos.pos_invoices WHERE id=$1",
            )
            .bind(first.pos_invoice_id),
        )
        .await
        .expect("read ticket header")
    })
    .await;
    assert_eq!(status, "draft");
    assert_eq!(paid, d("100000.00"));
    let fired = rec.events.lock().unwrap().iter().any(|e| matches!(
        e, PosEvent::PosTenderCompleted(t) if t.pos_invoice_id == first.pos_invoice_id
    ));
    assert!(fired, "the fire-and-forget sink still fires alongside the durable staging");

    l.teardown().await;
}

/// The failure path: when outbox staging is configured but the write fails, the SYNC fails —
/// never a silent success with a lost durable event. The denied schema (table present, INSERT not
/// granted to the app role) makes staging fail deterministically; the whole replay must roll back
/// (no ticket row) and the sink must stay silent.
#[tokio::test]
async fn staging_failure_fails_the_sync_never_a_silent_success() {
    let Some(l) = lab().await else { return };
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, session, tax) = seed_register(&l.admin, company).await;

    let rec = Recorder::default();
    let w = PosWriteService::with_sink(l.app.clone(), std::sync::Arc::new(rec.clone()))
        .with_outbox(OUTBOX_SCHEMA_DENIED);

    let err = w
        .sync_from_ui(
            sync(company, Uuid::new_v4(), prof, session,
                vec![sync_line(Uuid::new_v4(), item, "1", "50000")],
                vec![tender(Uuid::new_v4(), "cash", "50000")]),
            &tax, &StubBilling::default(), &StubPayment,
        )
        .await
        .expect_err("a staging failure must fail the replay");
    // The staging error rides the replay's own transaction as a typed DB error (the same mapping
    // the online add_tender path uses: the outbox error is wrapped into a protocol-level sqlx
    // error carrying its message).
    let msg = match &err {
        PosError::Db(sqlx::Error::Protocol(p)) => p.clone(),
        other => panic!("expected the outbox staging failure to propagate as a DB error, got {other:?}"),
    };
    assert!(
        msg.to_lowercase().contains("permission denied"),
        "expected the underlying permission-denied staging failure to surface, got: {msg}"
    );

    // Atomic: nothing from this replay survived — no ticket, no staged event, no sink fire. The
    // counts are read THROUGH the fence (scoped) so they prove presence/absence, not blindness.
    let tickets: i64 = company_scope::with_company_scope(Some(company), async {
        company_scope::fetch_one_scalar_scoped(
            &l.admin,
            sqlx::query_scalar("SELECT count(*) FROM pos.pos_invoices WHERE company_id=$1")
                .bind(company),
        )
        .await
        .expect("count tickets")
    })
    .await;
    assert_eq!(tickets, 0, "the failed replay must roll back entirely");
    let staged: i64 = company_scope::with_company_scope(Some(company), async {
        company_scope::fetch_one_scalar_scoped(
            &l.admin,
            sqlx::query_scalar(&format!(
                "SELECT count(*) FROM {OUTBOX_SCHEMA_DENIED}.outbox_events WHERE company_id=$1"
            ))
            .bind(company),
        )
        .await
        .expect("count denied-schema events")
    })
    .await;
    assert_eq!(staged, 0, "no event may be staged by a failed replay");
    assert!(rec.events.lock().unwrap().is_empty(), "the sink stays silent when the replay fails");

    l.teardown().await;
}
