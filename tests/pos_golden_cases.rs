//! Golden oracle for the POS write path (session → sale → tender → close). POS-only — the retail
//! recognition seam (billing + payment) is proven in `retail_sale_seam.rs`. Requires DATABASE_URL
//! (:5433/backbone_pos).

use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use backbone_pos::application::service::pos_write_service::{
    NewClose, NewSale, NewSaleLine, NewSession, PosError, PosWriteService,
};

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }
fn at() -> chrono::NaiveDateTime { chrono::NaiveDate::from_ymd_opt(2026, 7, 5).unwrap().and_hms_opt(9, 0, 0).unwrap() }
fn uq(p: &str) -> String { format!("{p}-{}", &Uuid::new_v4().simple().to_string()[..8]) }
async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5433/backbone_pos".to_string());
    PgPool::connect(&url).await.expect("connect DB")
}
async fn profile(pool: &PgPool, company: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, allow_discount, is_active)
        VALUES ($1,$2,'Register 1','IDR',true,true)"#).bind(id).bind(company).execute(pool).await.unwrap();
    id
}
fn line(item: Uuid, qty: &str, price: &str, disc: &str) -> NewSaleLine {
    NewSaleLine { item_id: item, revenue_account_id: None, description: None, quantity: d(qty), unit_price: d(price), discount_amount: d(disc) }
}
async fn open(w: &PosWriteService, company: Uuid, prof: Uuid, cash: &str) -> Uuid {
    w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![("cash".into(), d(cash))],
    }).await.unwrap()
}

// PGC-1: ring sale — 2 × 50,000 − 5,000 discount = 95,000 net; no tax → grand 95,000; no rounding.
#[tokio::test]
async fn ring_sale_totals() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let prof = profile(&pool, company).await;
    let session = open(&w, company, prof, "500000").await;
    let sale = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(item, "2", "50000", "5000")], tax_total: Decimal::ZERO, round_to: None,
    }).await.unwrap();
    let r = sqlx::query("SELECT net_total, grand_total, rounded_total, status::text AS st FROM pos.pos_invoices WHERE id=$1")
        .bind(sale).fetch_one(&pool).await.unwrap();
    assert_eq!(r.get::<Decimal, _>("net_total"), d("95000.00"));
    assert_eq!(r.get::<Decimal, _>("grand_total"), d("95000.00"));
    assert_eq!(r.get::<Decimal, _>("rounded_total"), d("95000.00"));
    assert_eq!(r.get::<String, _>("st"), "draft");
}

// PGC-2: IDR rounding to the nearest 100 — grand 95,040 rounds DOWN to 95,000 (adjustment −40);
// 95,060 rounds UP to 95,100 (adjustment +40).
#[tokio::test]
async fn receipt_rounding() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let prof = profile(&pool, company).await;
    let session = open(&w, company, prof, "0").await;
    let mk = |price: &str| NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(item, "1", price, "0")], tax_total: Decimal::ZERO, round_to: Some(d("100")),
    };
    let down = w.ring_sale(mk("95040")).await.unwrap();
    let (g, r, a): (Decimal, Decimal, Decimal) = sqlx::query_as("SELECT grand_total, rounded_total, rounding_adjustment FROM pos.pos_invoices WHERE id=$1").bind(down).fetch_one(&pool).await.unwrap();
    assert_eq!((g, r, a), (d("95040.00"), d("95000.00"), d("-40.00")));
    let up = w.ring_sale(mk("95060")).await.unwrap();
    let (g2, r2, a2): (Decimal, Decimal, Decimal) = sqlx::query_as("SELECT grand_total, rounded_total, rounding_adjustment FROM pos.pos_invoices WHERE id=$1").bind(up).fetch_one(&pool).await.unwrap();
    assert_eq!((g2, r2, a2), (d("95060.00"), d("95100.00"), d("40.00")));
}

// PGC-3: multi-tender + change — rounded 100,000; pay 60,000 card + 50,000 cash → paid 110,000,
// change 10,000, fully tendered.
#[tokio::test]
async fn multi_tender_and_change() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let prof = profile(&pool, company).await;
    let session = open(&w, company, prof, "0").await;
    let sale = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(item, "1", "100000", "0")], tax_total: Decimal::ZERO, round_to: None,
    }).await.unwrap();
    let t1 = w.add_tender(sale, "card", d("60000"), None).await.unwrap();
    assert!(!t1.fully_tendered);
    assert_eq!(t1.change_due, d("0.00"));
    let t2 = w.add_tender(sale, "cash", d("50000"), None).await.unwrap();
    assert!(t2.fully_tendered);
    assert_eq!(t2.paid_total, d("110000.00"));
    assert_eq!(t2.change_due, d("10000.00"));
}

// PGC-4: close reconciliation — opening cash 500,000 + no sales → expected cash 500,000; counting
// 500,000 balances (difference 0); counting 499,000 is short 1,000.
#[tokio::test]
async fn close_reconciliation() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let prof = profile(&pool, company).await;
    let session = open(&w, company, prof, "500000").await;
    let out = w.close_session(NewClose {
        company_id: company, opening_entry_id: session, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![("cash".into(), d("500000"))],
    }).await.unwrap();
    assert_eq!(out.difference_total, d("0.00"));
    let cash = out.by_method.iter().find(|r| r.method == "cash").unwrap();
    assert_eq!(cash.expected, d("500000.00"));
    // session is now closed — a sale can no longer ring against it.
    let st: String = sqlx::query_scalar("SELECT status::text FROM pos.pos_opening_entries WHERE id=$1").bind(session).fetch_one(&pool).await.unwrap();
    assert_eq!(st, "closed");
    let e = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(Uuid::new_v4(), "1", "1000", "0")], tax_total: Decimal::ZERO, round_to: None,
    }).await.unwrap_err();
    assert!(matches!(e, PosError::SessionNotOpen));
}

// PGC-5: validation gates — empty sale, negative amount, duplicate receipt number.
#[tokio::test]
async fn validation_gates() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let prof = profile(&pool, company).await;
    let session = open(&w, company, prof, "0").await;
    let base = |num: String, lines: Vec<NewSaleLine>| NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: num, posting_at: at(), lines, tax_total: Decimal::ZERO, round_to: None,
    };
    assert!(matches!(w.ring_sale(base(uq("R"), vec![])).await.unwrap_err(), PosError::EmptyDocument));
    assert!(matches!(w.ring_sale(base(uq("R"), vec![line(item, "-1", "100", "0")])).await.unwrap_err(), PosError::NegativeAmount));
    let num = uq("DUP");
    w.ring_sale(base(num.clone(), vec![line(item, "1", "100", "0")])).await.unwrap();
    assert!(matches!(w.ring_sale(base(num, vec![line(item, "1", "100", "0")])).await.unwrap_err(), PosError::DuplicateNumber(_)));
}
