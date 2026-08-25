//! Receipt assembly + monospace rendering, including the server-computed PPN line and the derived
//! invoiced flag. POS-only. Requires DATABASE_URL (:5433/backbone_pos).

mod support;
use support::{at, d, pool, uq};

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_pos::application::service::pos_write_service::{
    NewSale, NewSaleLine, NewSession, PosError, PosWriteService,
};

#[tokio::test]
async fn receipt_renders_lines_ppn_total_change_and_invoiced_flag() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    // PKP register: one 11% template (document-grade tax via the port).
    let (prof, tax) = support::profile_at_rate(&pool, company, "0.11").await;

    let session = w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![],
    }).await.unwrap();
    let sale = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None,
        receipt_number: uq("R"), posting_at: at(),
        lines: vec![NewSaleLine { item_id: Uuid::new_v4(), revenue_account_id: None, description: Some("Kopi Susu".into()), quantity: d("2"), unit_price: d("50000"), course: None, discount_amount: Decimal::ZERO }],
    }, &tax).await.unwrap();
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
    // The invoiced flag is DERIVED from the billing link — a ticket that has not been through
    // recognition has no invoice, so the slip says so without any stored state column.
    assert!(!r.is_invoiced, "a draft ticket has no billing invoice yet");

    // The rendered slip carries the line, the PPN line (legally required for a PKP receipt), total + change.
    let text = r.render_text();
    for needle in ["Kopi Susu", "PPN 11%", "TOTAL", "Change", "Register 1"] {
        assert!(text.contains(needle), "receipt slip must contain {needle:?}:\n{text}");
    }

    // Tenant scoping — another company cannot pull this receipt.
    assert!(matches!(w.receipt(Uuid::new_v4(), sale).await.unwrap_err(), PosError::InvoiceNotFound(_)));

    // After recognition the flag flips WITHOUT any state column: the derived read is the billing link.
    // (Recognition drives billing through the port — the POS-only stub below satisfies the seam. The
    // register needs its GL accounts + walk-in customer for recognition to run.)
    sqlx::query(r#"UPDATE pos.pos_profiles SET receivable_account_id=$2, income_account_id=$3, cash_account_id=$4, default_customer_id=$5, tax_account_id=$6 WHERE id=$1"#)
        .bind(prof).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4())
        .execute(&pool).await.unwrap();
    let billing = support::StubBilling { invoice: Uuid::new_v4(), ..Default::default() };
    w.recognize_sale(sale, &billing, &support::StubPayment, None).await.unwrap();
    let r2 = w.receipt(company, sale).await.unwrap();
    assert!(r2.is_invoiced, "a recognised ticket reads as invoiced (derived from the billing link)");
}
