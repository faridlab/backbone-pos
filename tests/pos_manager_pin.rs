//! Manager-PIN credentials (PSX-4): argon2-hashed server-side, verified on every privileged
//! mutation, persisted failure counters + lockout, config-driven windows, and a per-source throttle
//! ring. Also proves the credential-blind CRUD surface can never mint a working credential.
//! Requires DATABASE_URL (:5433/backbone_pos).

mod support;
use support::{at, d, pool, RecordingVariance};

use uuid::Uuid;

use backbone_pos::application::service::pos_manager_pin::SetPin;
use backbone_pos::application::service::pos_write_service::{
    ManagerAuth, NewClose, NewSession, PinPolicy, PosError, PosWriteService,
};

fn pin(employee: Uuid, code: &str) -> ManagerAuth {
    ManagerAuth { employee_party_id: employee, pin: code.to_string() }
}

fn set(company: Uuid, employee: Uuid, code: &str, current: Option<ManagerAuth>) -> SetPin {
    SetPin { company_id: company, employee_party_id: employee, new_pin: code.to_string(), current, source_ip: None }
}

/// MP-1: the FIRST credential bootstraps without proof; every later change demands proof.
#[tokio::test]
async fn bootstrap_then_proof_required() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();

    // Bootstrap: no credential exists at the company yet.
    w.set_pin(set(company, a, "4321", None)).await.unwrap();
    w.verify_pin(company, a, "4321", None).await.unwrap();

    // A second manager's first PIN now requires proof — none given → refused.
    let e = w.set_pin(set(company, b, "5678", None)).await.unwrap_err();
    assert!(matches!(e, PosError::ManagerAuthRequired));

    // Wrong proof → refused (and the wrong attempt counts against the proving manager).
    let e = w.set_pin(set(company, b, "5678", Some(pin(a, "9999")))).await.unwrap_err();
    assert!(matches!(e, PosError::PinInvalid));

    // Right proof (another manager's) → the supervised set lands and verifies.
    w.set_pin(set(company, b, "5678", Some(pin(a, "4321")))).await.unwrap();
    w.verify_pin(company, b, "5678", None).await.unwrap();

    // Self-change: the manager's own current PIN is proof.
    w.set_pin(set(company, a, "6789", Some(pin(a, "4321")))).await.unwrap();
    w.verify_pin(company, a, "6789", None).await.unwrap();
}

/// MP-2: strength — digits only, policy length window.
#[tokio::test]
async fn weak_pins_never_reach_the_hash() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    for weak in ["123", "1234567890", "12ab", "0000 "] {
        let e = w.set_pin(set(company, Uuid::new_v4(), weak, None)).await.unwrap_err();
        assert!(matches!(e, PosError::WeakPin(_)), "PIN {weak:?} must be refused");
    }
    // A refused set_pin leaves no credential: nothing to verify against.
    let e = w.verify_pin(company, Uuid::new_v4(), "4321", None).await.unwrap_err();
    assert!(matches!(e, PosError::PinNotFound));
}

/// MP-3: wrong PINs fail closed, the failure counter persists, and crossing the threshold LOCKS the
/// credential with its unlock instant. A locked credential refuses even the CORRECT pin until the
/// window passes.
#[tokio::test]
async fn failures_lock_the_credential() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone()).with_pin_policy(PinPolicy {
        max_attempts: 3, lockout_secs: 3600, min_digits: 4, max_digits: 8, ip_window_secs: 60, ip_max_attempts: 100,
    });
    let company = Uuid::new_v4();
    let m = Uuid::new_v4();
    w.set_pin(set(company, m, "4321", None)).await.unwrap();

    assert!(matches!(w.verify_pin(company, m, "1111", None).await.unwrap_err(), PosError::PinInvalid));
    assert!(matches!(w.verify_pin(company, m, "2222", None).await.unwrap_err(), PosError::PinInvalid));
    // Third failure crosses the threshold → the LOCK is reported (with the unlock instant).
    match w.verify_pin(company, m, "3333", None).await.unwrap_err() {
        PosError::PinLocked { until } => assert!(until > chrono::Utc::now()),
        other => panic!("expected PinLocked, got {other:?}"),
    }
    // The counter persisted.
    let fails: i32 = sqlx::query_scalar("SELECT failed_attempts FROM pos.pos_manager_pins WHERE employee_party_id=$1")
        .bind(m).fetch_one(&pool).await.unwrap();
    assert_eq!(fails, 3);
    // Even the CORRECT pin is refused while locked.
    let e = w.verify_pin(company, m, "4321", None).await.unwrap_err();
    assert!(matches!(e, PosError::PinLocked { .. }));

    // The window passing re-opens the credential (simulate expiry by back-dating the lock).
    sqlx::query("UPDATE pos.pos_manager_pins SET locked_until = now() - interval '1 second' WHERE employee_party_id=$1")
        .bind(m).execute(&pool).await.unwrap();
    w.verify_pin(company, m, "4321", None).await.unwrap();
    // A successful verify resets the counter (the next wrong run starts from zero).
    let fails: i32 = sqlx::query_scalar("SELECT failed_attempts FROM pos.pos_manager_pins WHERE employee_party_id=$1")
        .bind(m).fetch_one(&pool).await.unwrap();
    assert_eq!(fails, 0);
}

/// MP-4: the per-source throttle ring — a source over its attempt budget is fenced BEFORE it can
/// touch any manager's counter (config-driven window, no production sleep needed to prove it).
#[tokio::test]
async fn per_source_throttle_fences_the_source() {
    let pool = pool().await;
    let w = PosWriteService::with_sink(pool.clone(), std::sync::Arc::new(backbone_pos::application::service::LoggingSink))
        .with_pin_policy(PinPolicy { max_attempts: 50, lockout_secs: 3600, min_digits: 4, max_digits: 8, ip_window_secs: 60, ip_max_attempts: 3 });
    let company = Uuid::new_v4();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    w.set_pin(set(company, a, "4321", None)).await.unwrap();
    w.set_pin(set(company, b, "5678", Some(pin(a, "4321")))).await.unwrap();

    let ip = "203.0.113.9";
    // Three attempts from the source are inside the budget (ip_max_attempts = 3)...
    let _ = w.verify_pin(company, a, "4321", Some(ip)).await.unwrap();
    let _ = w.verify_pin(company, b, "5678", Some(ip)).await.unwrap();
    let _ = w.verify_pin(company, a, "4321", Some(ip)).await.unwrap();
    // ...the fourth is over budget and fenced, even for a DIFFERENT manager + the CORRECT pin.
    let e = w.verify_pin(company, a, "4321", Some(ip)).await.unwrap_err();
    assert!(matches!(e, PosError::PinThrottled));
    // Another source is unaffected.
    w.verify_pin(company, a, "4321", Some("198.51.100.4")).await.unwrap();
    // The fenced manager's counter was never touched by the throttled call.
    let fails: i32 = sqlx::query_scalar("SELECT failed_attempts FROM pos.pos_manager_pins WHERE employee_party_id=$1")
        .bind(a).fetch_one(&pool).await.unwrap();
    assert_eq!(fails, 0);
}

/// MP-5: a credential row created through the credential-blind CRUD surface (no hash, the
/// non-verifying placeholder) can NEVER authenticate — the verify path fails closed as a wrong PIN,
/// not a server error.
#[tokio::test]
async fn crud_created_rows_never_authenticate() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let m = Uuid::new_v4();
    // Exactly what the generated CRUD create lands on (the DTO's placeholder).
    sqlx::query(r#"INSERT INTO pos.pos_manager_pins (id, company_id, employee_party_id, pin_hash, failed_attempts, metadata)
        VALUES ($1,$2,$3,'(set via the guarded set-pin verb)',0,'{}'::jsonb)"#)
        .bind(Uuid::new_v4()).bind(company).bind(m).execute(&pool).await.unwrap();
    for guess in ["4321", "0000", "12345678"] {
        let e = w.verify_pin(company, m, guess, None).await.unwrap_err();
        assert!(matches!(e, PosError::PinInvalid), "a placeholder hash must fail closed as wrong-PIN");
    }
}

/// MP-6: the RESPONSE surface never carries the hash or the attempt-source address — serialize the
/// DTO and inspect the wire.
#[tokio::test]
async fn response_dto_is_credential_blind() {
    use backbone_pos::domain::entity::PosManagerPin;
    let entity = PosManagerPin {
        id: Uuid::new_v4(),
        company_id: Uuid::new_v4(),
        employee_party_id: Uuid::new_v4(),
        pin_hash: "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$hash-bytes".into(),
        failed_attempts: 1,
        locked_until: None,
        last_attempt_at: None,
        last_attempt_ip: Some("203.0.113.9".into()),
        metadata: Default::default(),
    };
    let dto: backbone_pos::presentation::dto::pos_manager_pin_dto::PosManagerPinResponseDto = entity.into();
    let wire = serde_json::to_string(&dto).unwrap();
    assert!(!wire.contains("argon2"), "the hash never appears on the wire: {wire}");
    assert!(!wire.contains("203.0.113.9"), "the attempt-source address never appears on the wire: {wire}");
    assert!(!wire.contains("pinHash"), "no pin-hash key exists at all: {wire}");
    // The admin-visible lockout state IS present (that is the point of the surface).
    assert!(wire.contains("failedAttempts"));
}

/// MP-7: privileged-verb integration — closing the till verifies the PIN server-side on EVERY call,
/// and a refused verify leaves the session untouched (proved end-to-end in pos_golden_cases PGC-4;
/// this probe pins the no-credential case).
#[tokio::test]
async fn close_without_any_credential_refuses() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let prof = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, tax_template_ids, allow_discount, status)
           VALUES ($1,$2,'Register 1','IDR','[]'::jsonb,true,'active')"#,
    )
    .bind(prof).bind(company)
    .execute(&pool).await.unwrap();
    let session = w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![("cash".into(), d("0"))],
    }).await.unwrap();
    let e = w.close_session(NewClose {
        company_id: company, opening_entry_id: session, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![],
        manager: pin(Uuid::new_v4(), "4321"), source_ip: None,
    }, &RecordingVariance::default()).await.unwrap_err();
    assert!(matches!(e, PosError::PinNotFound), "a manager with no credential cannot close");
    let st: String = sqlx::query_scalar("SELECT status::text FROM pos.pos_opening_entries WHERE id=$1")
        .bind(session).fetch_one(&pool).await.unwrap();
    assert_eq!(st, "open");
}
