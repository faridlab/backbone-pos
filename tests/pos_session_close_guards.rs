//! A session closes once, and nothing lands on it after: a tender on a ticket whose session is no
//! longer open is refused, and a second closing entry for one session is refused by the database.
//! POS-only. Requires DATABASE_URL (:5433/backbone_pos) migrated to the module's migrations.

mod support;
use support::{at, d, pool, StubBilling, StubPayment, TestTax};

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_pos::application::service::pos_write_service::{NewSession, NewSyncSale, PosError, PosWriteService, SyncSaleLine};

async fn register(pool: &sqlx::PgPool) -> (Uuid, TestTax) {
    let template = Uuid::new_v4();
    let id = Uuid::new_v4();
    sqlx::query(r#"INSERT INTO pos.pos_profiles (id, name, currency, tax_template_ids, receivable_account_id, income_account_id, cash_account_id, default_customer_id, tax_account_id, allow_discount, status)
        VALUES ($1,'Register 1','IDR',$2,$3,$4,$5,$6,$7,true,'active')"#)
        .bind(id).bind(serde_json::json!([template.to_string()]))
        .bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4())
        .execute(pool).await.unwrap();
    (id, TestTax::with_rate(template, "0"))
}

async fn open(w: &PosWriteService, prof: Uuid) -> Uuid {
    w.open_session(NewSession {
        pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![],
    }).await.unwrap()
}

/// A draft ticket of 100,000 with no tender yet, on a fresh session.
async fn draft(w: &PosWriteService, prof: Uuid, tax: &TestTax) -> (Uuid, Uuid) {
    let session = open(w, prof).await;
    let ticket = w.sync_from_ui(NewSyncSale {
        client_uuid: Uuid::new_v4(), pos_profile_id: prof, opening_entry_id: session,
        rescue_opening_entry_id: None, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None, posting_at: at(),
        lines: vec![SyncSaleLine {
            client_uuid: Uuid::new_v4(), item_id: Uuid::new_v4(), revenue_account_id: None, description: None,
            course: None, quantity: d("1"), unit_price: d("100000"), discount_amount: Decimal::ZERO,
        }],
        tenders: vec![], refund_of_client_uuid: None, manager: None, source_ip: None,
    }, tax, &StubBilling::default(), &StubPayment).await.unwrap().pos_invoice_id;
    (session, ticket)
}

#[tokio::test]
async fn a_tender_on_a_session_that_is_no_longer_open_is_refused() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (prof, tax) = register(&pool).await;
    let (session, ticket) = draft(&w, prof, &tax).await;
    sqlx::query("UPDATE pos.pos_opening_entries SET status = 'closed'::pos_session_status WHERE id = $1")
        .bind(session).execute(&pool).await.unwrap();

    let refused = w.add_tender(ticket, "cash", d("100000"), None).await;
    assert!(matches!(refused, Err(PosError::SessionNotOpen)), "{refused:?}");
    let tenders: i64 = sqlx::query_scalar("SELECT count(*) FROM pos.pos_payments WHERE pos_invoice_id = $1")
        .bind(ticket).fetch_one(&pool).await.unwrap();
    assert_eq!(tenders, 0, "a refused tender writes nothing");
}

#[tokio::test]
async fn a_second_closing_entry_for_one_session_is_refused() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (prof, _) = register(&pool).await;
    let session = open(&w, prof).await;
    let insert = || sqlx::query(r#"INSERT INTO pos.pos_closing_entries
        (id, pos_profile_id, opening_entry_id, closed_at, cashier_party_id, totals_by_method, grand_total, invoice_count, difference_total, status)
        VALUES ($1,$2,$3,$4,$5,'[]'::jsonb,0,0,0,'submitted'::pos_closing_status)"#)
        .bind(Uuid::new_v4()).bind(prof).bind(session).bind(at()).bind(Uuid::new_v4());
    insert().execute(&pool).await.unwrap();
    let second = insert().execute(&pool).await;
    let code = second.err().and_then(|e| e.as_database_error().and_then(|d| d.code().map(|c| c.to_string())));
    assert_eq!(code.as_deref(), Some("23505"), "one closing entry per session");
}
