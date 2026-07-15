//! Receipt assembly + monospace rendering, including the server-computed PPN line. POS-only. Requires
//! DATABASE_URL (:5433/backbone_pos).

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_pos::application::service::pos_write_service::{
    NewSale, NewSaleLine, NewSession, PosError, PosWriteService,
};

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }
fn at() -> chrono::NaiveDateTime { chrono::NaiveDate::from_ymd_opt(2026, 7, 14).unwrap().and_hms_opt(9, 0, 0).unwrap() }
async fn pool() -> sqlx::PgPool {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5433/backbone_pos".to_string());
    sqlx::PgPool::connect(&url).await.expect("connect DB")
}

#[tokio::test]
async fn receipt_renders_lines_ppn_total_and_change() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let prof = Uuid::new_v4();
    // PKP register: 11% PPN.
    sqlx::query("INSERT INTO pos.pos_profiles (id, company_id, name, currency, tax_account_id, tax_rate, allow_discount, is_active) VALUES ($1,$2,'Register 1','IDR',$3,0.1100,true,true)")
        .bind(prof).bind(company).bind(Uuid::new_v4()).execute(&pool).await.unwrap();

    let session = w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![],
    }).await.unwrap();
    let sale = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: format!("R-{}", &Uuid::new_v4().simple().to_string()[..8]), posting_at: at(),
        lines: vec![NewSaleLine { item_id: Uuid::new_v4(), revenue_account_id: None, description: Some("Kopi Susu".into()), quantity: d("2"), unit_price: d("50000"), discount_amount: Decimal::ZERO }],
        tax_total: Decimal::ZERO, round_to: None,
    }).await.unwrap();
    w.add_tender(sale, "cash", d("120000"), None).await.unwrap();

    let r = w.receipt(company, sale).await.unwrap();
    // Money breakdown (server PPN): net 100k, PPN 11k, grand 111k, change 120k − 111k = 9k.
    assert_eq!(r.net_total, d("100000.00"));
    assert_eq!(r.tax_total, d("11000.00"));
    assert_eq!(r.grand_total, d("111000.00"));
    assert_eq!(r.change_due, d("9000.00"));
    assert_eq!(r.lines.len(), 1);
    assert_eq!(r.lines[0].description, "Kopi Susu");
    assert_eq!(r.tenders.len(), 1);

    // The rendered slip carries the line, the PPN line (legally required for a PKP receipt), total + change.
    let text = r.render_text();
    for needle in ["Kopi Susu", "PPN 11%", "TOTAL", "Change", "Register 1"] {
        assert!(text.contains(needle), "receipt slip must contain {needle:?}:\n{text}");
    }

    // Tenant scoping — another company cannot pull this receipt.
    assert!(matches!(w.receipt(Uuid::new_v4(), sale).await.unwrap_err(), PosError::InvoiceNotFound(_)));
}
