//! Golden cases for non-sale cash drawer movements (pay-in / pay-out / drop / no-sale) and how
//! `close_session` folds them into the expected cash drawer. POS-only. Requires DATABASE_URL
//! (:5433/backbone_pos).

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_pos::application::service::pos_write_service::{
    NewCashMovement, NewClose, NewSession, PosError, PosWriteService,
};

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }
fn at() -> chrono::NaiveDateTime { chrono::NaiveDate::from_ymd_opt(2026, 7, 14).unwrap().and_hms_opt(9, 0, 0).unwrap() }
async fn pool() -> sqlx::PgPool {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5433/backbone_pos".to_string());
    sqlx::PgPool::connect(&url).await.expect("connect DB")
}
async fn open(w: &PosWriteService, company: Uuid, prof: Uuid, cash: &str) -> Uuid {
    w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![("cash".into(), d(cash))],
    }).await.unwrap()
}
fn mv(company: Uuid, prof: Uuid, session: Uuid, kind: &str, amount: &str) -> NewCashMovement {
    NewCashMovement {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, cashier_party_id: Uuid::new_v4(),
        movement_type: kind.into(), amount: d(amount), reason: Some(format!("{kind} test")), moved_at: at(),
    }
}

/// PCM-1: pay-in adds, pay-out and drop remove, no-sale is neutral — the drawer reconciles exactly.
#[tokio::test]
async fn cash_movements_fold_into_the_expected_drawer() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let prof = Uuid::new_v4();
    let session = open(&w, company, prof, "500000").await;

    w.record_cash_movement(mv(company, prof, session, "pay_in", "100000")).await.unwrap();
    w.record_cash_movement(mv(company, prof, session, "pay_out", "30000")).await.unwrap();
    w.record_cash_movement(mv(company, prof, session, "drop", "200000")).await.unwrap();
    w.record_cash_movement(mv(company, prof, session, "no_sale", "0")).await.unwrap();

    // expected cash = opening 500,000 + pay_in 100,000 − pay_out 30,000 − drop 200,000 = 370,000.
    let out = w.close_session(NewClose {
        company_id: company, opening_entry_id: session, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![("cash".into(), d("370000"))],
    }).await.unwrap();
    let cash = out.by_method.iter().find(|r| r.method == "cash").unwrap();
    assert_eq!(cash.expected, d("370000.00"), "movements must be reflected in expected cash");
    assert_eq!(out.difference_total, d("0.00"), "drawer balances once movements are recorded");
}

/// PCM-2: without recording the movements, the same physical drawer reads as a variance.
#[tokio::test]
async fn unrecorded_movements_show_as_variance() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let prof = Uuid::new_v4();
    let session = open(&w, company, prof, "500000").await;
    // A 200,000 drop happened physically but was NOT recorded; drawer holds 300,000.
    let out = w.close_session(NewClose {
        company_id: company, opening_entry_id: session, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![("cash".into(), d("300000"))],
    }).await.unwrap();
    assert_eq!(out.difference_total, d("-200000.00"), "unrecorded drop reads as short");
}

/// PCM-3: validation + tenant/session scoping.
#[tokio::test]
async fn cash_movement_validation_and_scoping() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let other = Uuid::new_v4();
    let prof = Uuid::new_v4();
    let session = open(&w, company, prof, "0").await;

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
    w.close_session(NewClose { company_id: company, opening_entry_id: session, cashier_party_id: Uuid::new_v4(), closed_at: at(), counted: vec![] }).await.unwrap();
    assert!(matches!(w.record_cash_movement(mv(company, prof, session, "pay_in", "1000")).await.unwrap_err(),
        PosError::SessionNotOpen));
}
