//! Golden oracle for the POS write path (session → sale → tender → close). POS-only — the retail
//! recognition seam (billing + payment) is proven in `retail_sale_seam.rs`. Requires DATABASE_URL
//! (:5433/backbone_pos).
//!
//! Tax on every ring resolves through the register's templates via `PosTaxComputePort` (the
//! in-test `TestTax` here); the register's own config decides cash rounding.

mod support;
use support::{at, d, manager_with_pin, pool, seed_profile, uq, RecordingVariance, TestTax};

use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use backbone_pos::application::service::pos_write_service::{
    NewClose, NewSale, NewSaleLine, NewSession, PosError, PosWriteService,
};

async fn profile(pool: &PgPool, company: Uuid) -> (Uuid, TestTax) {
    support::profile_at_rate(pool, company, "0").await
}
fn line(item: Uuid, qty: &str, price: &str, disc: &str) -> NewSaleLine {
    NewSaleLine { item_id: item, revenue_account_id: None, description: None, quantity: d(qty), unit_price: d(price), course: None, discount_amount: d(disc) }
}
async fn open(w: &PosWriteService, company: Uuid, prof: Uuid, cash: &str) -> Uuid {
    w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![("cash".into(), d(cash))],
    }).await.unwrap()
}
/// A register configured to round the pay-to total to the nearest 100 (IDR receipt rounding).
async fn rounding_profile(pool: &PgPool, company: Uuid) -> (Uuid, TestTax) {
    let template = Uuid::new_v4();
    let prof = seed_profile(pool, company, &[template], Some(("half_up", d("100")))).await;
    (prof, TestTax::with_rate(template, "0"))
}

// PGC-1: ring sale — 2 × 50,000 − 5,000 discount = 95,000 net; zero-rate template → no tax → grand
// 95,000; register not rounding-configured → no rounding.
#[tokio::test]
async fn ring_sale_totals() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = profile(&pool, company).await;
    let session = open(&w, company, prof, "500000").await;
    let sale = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(item, "2", "50000", "5000")],
    }, &tax).await.unwrap();
    let r = sqlx::query("SELECT net_total, grand_total, rounded_total, status::text AS st FROM pos.pos_invoices WHERE id=$1")
        .bind(sale).fetch_one(&pool).await.unwrap();
    assert_eq!(r.get::<Decimal, _>("net_total"), d("95000.00"));
    assert_eq!(r.get::<Decimal, _>("grand_total"), d("95000.00"));
    assert_eq!(r.get::<Decimal, _>("rounded_total"), d("95000.00"));
    assert_eq!(r.get::<String, _>("st"), "draft");
}

// PGC-1b: a ring pairs the session with the register the ticket names — register A's config with
// register B's open drawer is refused (cash would misattribute between tills).
#[tokio::test]
async fn ring_sale_must_stay_on_its_register() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof_a, tax) = profile(&pool, company).await;
    let (prof_b, _) = profile(&pool, company).await;
    let session_b = open(&w, company, prof_b, "0").await;
    let e = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof_a, opening_entry_id: session_b, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(item, "1", "50000", "0")],
    }, &tax).await.unwrap_err();
    assert!(matches!(e, PosError::SessionRegisterMismatch), "cross-register ring refused, got {e:?}");
    let tickets: i64 = sqlx::query_scalar("SELECT count(*) FROM pos.pos_invoices WHERE company_id=$1")
        .bind(company).fetch_one(&pool).await.unwrap();
    assert_eq!(tickets, 0, "nothing was written");
}

// PGC-2: IDR rounding is REGISTER config now (the client no longer sends a step): a half_up register
// with unit 100 rounds grand 95,040 DOWN to 95,000 (adjustment −40) and 95,060 UP to 95,100 (+40).
#[tokio::test]
async fn receipt_rounding_comes_from_the_register_config() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = rounding_profile(&pool, company).await;
    let session = open(&w, company, prof, "0").await;
    let mk = |price: &str| NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(item, "1", price, "0")],
    };
    let down = w.ring_sale(mk("95040"), &tax).await.unwrap();
    let (g, r, a): (Decimal, Decimal, Decimal) = sqlx::query_as("SELECT grand_total, rounded_total, rounding_adjustment FROM pos.pos_invoices WHERE id=$1").bind(down).fetch_one(&pool).await.unwrap();
    assert_eq!((g, r, a), (d("95040.00"), d("95000.00"), d("-40.00")));
    let up = w.ring_sale(mk("95060"), &tax).await.unwrap();
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
    let (prof, tax) = profile(&pool, company).await;
    let session = open(&w, company, prof, "0").await;
    let sale = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(item, "1", "100000", "0")],
    }, &tax).await.unwrap();
    let t1 = w.add_tender(sale, "card", d("60000"), None).await.unwrap();
    assert!(!t1.fully_tendered);
    assert_eq!(t1.change_due, d("0.00"));
    let t2 = w.add_tender(sale, "cash", d("50000"), None).await.unwrap();
    assert!(t2.fully_tendered);
    assert_eq!(t2.paid_total, d("110000.00"));
    assert_eq!(t2.change_due, d("10000.00"));
}

// PGC-4: close reconciliation — opening cash 500,000 + no sales → expected cash 500,000; counting
// 500,000 balances (difference 0, NO variance booked); the close verifies the manager's PIN
// server-side. After the close the session is closed and refuses further sales.
#[tokio::test]
async fn close_reconciliation_and_manager_gate() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let (prof, tax) = profile(&pool, company).await;
    let session = open(&w, company, prof, "500000").await;
    let manager = manager_with_pin(&w, company, "4321").await;
    let variance = RecordingVariance::default();

    // A wrong PIN refuses the close BEFORE anything is written — the session survives.
    let wrong = w.close_session(NewClose {
        company_id: company, opening_entry_id: session, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![("cash".into(), d("500000"))],
        manager: backbone_pos::application::service::pos_write_service::ManagerAuth {
            employee_party_id: manager.employee_party_id, pin: "9999".into(),
        },
        source_ip: None,
    }, &variance).await.unwrap_err();
    assert!(matches!(wrong, PosError::PinInvalid), "close is privileged: wrong PIN refuses it");
    let st0: String = sqlx::query_scalar("SELECT status::text FROM pos.pos_opening_entries WHERE id=$1").bind(session).fetch_one(&pool).await.unwrap();
    assert_eq!(st0, "open", "a refused close must leave the session untouched");
    assert!(variance.booking_for(session).is_none());

    let out = w.close_session(NewClose {
        company_id: company, opening_entry_id: session, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![("cash".into(), d("500000"))],
        manager, source_ip: None,
    }, &variance).await.unwrap();
    assert_eq!(out.difference_total, d("0.00"));
    let cash = out.by_method.iter().find(|r| r.method == "cash").unwrap();
    assert_eq!(cash.expected, d("500000.00"));
    assert!(out.variance.is_none(), "a balanced drawer books no journal");
    assert!(variance.booking_for(session).is_none(), "the variance port is not driven for a zero difference");
    // session is now closed — a sale can no longer ring against it.
    let st: String = sqlx::query_scalar("SELECT status::text FROM pos.pos_opening_entries WHERE id=$1").bind(session).fetch_one(&pool).await.unwrap();
    assert_eq!(st, "closed");
    let e = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(Uuid::new_v4(), "1", "1000", "0")],
    }, &tax).await.unwrap_err();
    assert!(matches!(e, PosError::SessionNotOpen));
}

// PGC-5: validation gates — empty sale, negative amount, duplicate receipt number, and the typed
// refusal for a register with no tax templates configured.
#[tokio::test]
async fn validation_gates() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = profile(&pool, company).await;
    let session = open(&w, company, prof, "0").await;
    let base = |num: String, lines: Vec<NewSaleLine>| NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None,
        receipt_number: num, posting_at: at(), lines,
    };
    assert!(matches!(w.ring_sale(base(uq("R"), vec![]), &tax).await.unwrap_err(), PosError::EmptyDocument));
    assert!(matches!(w.ring_sale(base(uq("R"), vec![line(item, "-1", "100", "0")]), &tax).await.unwrap_err(), PosError::NegativeAmount));
    let num = uq("DUP");
    w.ring_sale(base(num.clone(), vec![line(item, "1", "100", "0")]), &tax).await.unwrap();
    assert!(matches!(w.ring_sale(base(num, vec![line(item, "1", "100", "0")]), &tax).await.unwrap_err(), PosError::DuplicateNumber(_)));

    // A register whose templates were never configured refuses the ring with the typed error —
    // NULL/empty cannot silently mean "no tax" (a zero-RATE template is the non-PKP expression).
    let bare = Uuid::new_v4();
    sqlx::query("INSERT INTO pos.pos_profiles (id, company_id, name, currency, allow_discount, status) VALUES ($1,$2,'Bare','IDR',true,'active')")
        .bind(bare).bind(company).execute(&pool).await.unwrap();
    let s2 = open(&w, company, bare, "0").await;
    let e = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: bare, opening_entry_id: s2, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(item, "1", "1000", "0")],
    }, &tax).await.unwrap_err();
    assert!(matches!(e, PosError::ProfileTaxTemplatesMissing(_)));
}

// A register holds ONE open session: a second open on the same register is the typed refusal
// mapped from the one-open-session partial unique (DB arm) — never an internal error.
#[tokio::test]
async fn second_open_on_same_register_refuses() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let (prof, _tax) = profile(&pool, company).await;
    open(&w, company, prof, "0").await;
    let e = w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![],
    }).await.unwrap_err();
    assert!(matches!(e, PosError::SessionAlreadyOpen));
    assert_eq!(e.http_status(), 422);
}
