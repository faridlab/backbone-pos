//! Golden cases for non-sale cash drawer movements (pay-in / pay-out / drop / no-sale) and how
//! `close_session` folds them into the expected cash drawer. POS-only. Requires DATABASE_URL
//! (:5433/backbone_pos).
//!
//! Every close verifies a manager PIN (server-side) and books a non-zero drawer difference through
//! the `PosCashVariancePort` (the in-test `RecordingVariance` here).

mod support;
use support::{at, d, manager_with_pin, pool, RecordingVariance};

use uuid::Uuid;

use backbone_pos::application::service::pos_write_service::{
    NewCashMovement, NewClose, NewSession, PosError, PosWriteService,
};
use backbone_pos::application::service::pos_write_service::XReport;

fn mv(company: Uuid, prof: Uuid, session: Uuid, kind: &str, amount: &str) -> NewCashMovement {
    NewCashMovement {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, cashier_party_id: Uuid::new_v4(),
        movement_type: kind.into(), amount: d(amount), reason: Some(format!("{kind} test")), moved_at: at(),
    }
}

/// Open a session on a bare register (a profile row with no configuration — no sale is ever rung
/// here) + set up the manager credential the close verifies. Opening requires the register to exist
/// in the tenant, so the fixture seeds one.
async fn setup(pool: &sqlx::PgPool, w: &PosWriteService, company: Uuid, float: &str) -> (Uuid, Uuid, backbone_pos::application::service::pos_write_service::ManagerAuth) {
    let prof = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, tax_template_ids, allow_discount, status)
           VALUES ($1,$2,'Register 1','IDR','[]'::jsonb,true,'active')"#,
    )
    .bind(prof).bind(company)
    .execute(pool).await.unwrap();
    let session = w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![("cash".into(), d(float))],
    }).await.unwrap();
    let manager = manager_with_pin(w, company, "4321").await;
    (prof, session, manager)
}

fn close(company: Uuid, session: Uuid, manager: backbone_pos::application::service::pos_write_service::ManagerAuth, counted: &str) -> NewClose {
    NewClose {
        company_id: company, opening_entry_id: session, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![("cash".into(), d(counted))],
        manager, source_ip: None,
    }
}

/// A register with the two accounts a variance close needs: the drawer's cash account and the
/// difference/write-off account the correction books against.
async fn variance_profile(pool: &sqlx::PgPool, company: Uuid, template: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, tax_template_ids, cash_account_id, write_off_account_id, allow_discount, status)
        VALUES ($1,$2,'Register 1','IDR',$3,$4,$5,true,'active')"#)
        .bind(id).bind(company)
        .bind(serde_json::json!([template.to_string()]))
        .bind(Uuid::new_v4()).bind(Uuid::new_v4())
        .execute(pool).await.unwrap();
    id
}

/// PCM-1: pay-in adds, pay-out and drop remove, no-sale is neutral — the drawer reconciles exactly,
/// a balanced close books no variance journal.
#[tokio::test]
async fn cash_movements_fold_into_the_expected_drawer() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let (prof, session, manager) = setup(&pool, &w, company, "500000").await;
    let variance = RecordingVariance::default();

    w.record_cash_movement(mv(company, prof, session, "pay_in", "100000")).await.unwrap();
    w.record_cash_movement(mv(company, prof, session, "pay_out", "30000")).await.unwrap();
    w.record_cash_movement(mv(company, prof, session, "drop", "200000")).await.unwrap();
    w.record_cash_movement(mv(company, prof, session, "no_sale", "0")).await.unwrap();

    // expected cash = opening 500,000 + pay_in 100,000 − pay_out 30,000 − drop 200,000 = 370,000.
    let out = w.close_session(close(company, session, manager, "370000"), &variance).await.unwrap();
    let cash = out.by_method.iter().find(|r| r.method == "cash").unwrap();
    assert_eq!(cash.expected, d("370000.00"), "movements must be reflected in expected cash");
    assert_eq!(out.difference_total, d("0.00"), "drawer balances once movements are recorded");
    assert!(variance.booking_for(session).is_none(), "a balanced drawer drives the variance port nowhere");
}

/// XR-1: the X-report shows the same expected drawer as a close (incl. cash movements) WITHOUT closing
/// the session, and is idempotent (a cashier can pull it repeatedly mid-shift).
#[tokio::test]
async fn x_report_reads_the_drawer_without_closing() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let other = Uuid::new_v4();
    let (prof, session, manager) = setup(&pool, &w, company, "500000").await;
    let variance = RecordingVariance::default();
    w.record_cash_movement(mv(company, prof, session, "pay_in", "100000")).await.unwrap();
    w.record_cash_movement(mv(company, prof, session, "drop", "200000")).await.unwrap();

    let check = |r: XReport| {
        let cash = r.by_method.iter().find(|m| m.method == "cash").unwrap();
        assert_eq!(cash.expected, d("400000.00"), "expected cash = 500k + 100k − 200k");
        assert_eq!(r.invoice_count, 0);
    };
    check(w.x_report(company, session).await.unwrap());
    // idempotent — pulling again gives the same numbers, session stays OPEN.
    check(w.x_report(company, session).await.unwrap());
    let st: String = sqlx::query_scalar("SELECT status::text FROM pos.pos_opening_entries WHERE id=$1").bind(session).fetch_one(&pool).await.unwrap();
    assert_eq!(st, "open", "x-report must not close the session");

    // scoping: another tenant cannot read this session's drawer.
    assert!(matches!(w.x_report(other, session).await.unwrap_err(), PosError::SessionNotFound(_)));
    // a non-existent session.
    assert!(matches!(w.x_report(company, Uuid::new_v4()).await.unwrap_err(), PosError::SessionNotFound(_)));

    // after close, the X-report (open-only) is refused.
    w.close_session(close(company, session, manager, "400000"), &variance).await.unwrap();
    assert!(matches!(w.x_report(company, session).await.unwrap_err(), PosError::SessionNotOpen));
}

/// PCM-2: without recording the movements, the same physical drawer reads as a variance — and a
/// NON-ZERO difference now BOOKS through the variance port to the register's write-off account.
#[tokio::test]
async fn unrecorded_movements_show_as_variance_and_book() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let prof = variance_profile(&pool, company, Uuid::new_v4()).await;
    let session = w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![("cash".into(), d("500000"))],
    }).await.unwrap();
    let manager = manager_with_pin(&w, company, "4321").await;
    let variance = RecordingVariance::default();

    // A 200,000 drop happened physically but was NOT recorded; drawer holds 300,000.
    let out = w.close_session(close(company, session, manager, "300000"), &variance).await.unwrap();
    assert_eq!(out.difference_total, d("-200000.00"), "unrecorded drop reads as short");
    let booked = out.variance.expect("a non-zero difference must book a journal");
    assert_eq!(booked.amount, d("200000.00"));
    assert_eq!(booked.direction, backbone_pos::application::service::pos_ports::CashVarianceDirection::Short);
    assert_eq!(variance.booking_for(session), Some((d("200000.00"), backbone_pos::application::service::pos_ports::CashVarianceDirection::Short)));
    // The booked journal traces to this close's entry.
    let closing: Uuid = sqlx::query_scalar("SELECT id FROM pos.pos_closing_entries WHERE opening_entry_id=$1").bind(session).fetch_one(&pool).await.unwrap();
    assert_eq!(closing, out.closing_id);
}

/// PCM-3: validation + tenant/session scoping.
#[tokio::test]
async fn cash_movement_validation_and_scoping() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let other = Uuid::new_v4();
    let (prof, session, manager) = setup(&pool, &w, company, "0").await;
    let variance = RecordingVariance::default();

    // no_sale must carry amount 0.
    assert!(matches!(w.record_cash_movement(mv(company, prof, session, "no_sale", "5000")).await.unwrap_err(),
        PosError::InvalidCashMovement(_)));
    // pay_in must be strictly positive.
    assert!(matches!(w.record_cash_movement(mv(company, prof, session, "pay_in", "0")).await.unwrap_err(),
        PosError::InvalidCashMovement(_)));
    // unknown type rejected.
    assert!(matches!(w.record_cash_movement(mv(company, prof, session, "bogus", "1000")).await.unwrap_err(),
        PosError::InvalidCashMovement(_)));
    // another tenant cannot move cash in this session.
    assert!(matches!(w.record_cash_movement(mv(other, prof, session, "pay_in", "1000")).await.unwrap_err(),
        PosError::SessionNotOpen));

    // once closed, no more movements.
    w.close_session(NewClose {
        company_id: company, opening_entry_id: session, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![],
        manager, source_ip: None,
    }, &variance).await.unwrap();
    assert!(matches!(w.record_cash_movement(mv(company, prof, session, "pay_in", "1000")).await.unwrap_err(),
        PosError::SessionNotOpen));
}
