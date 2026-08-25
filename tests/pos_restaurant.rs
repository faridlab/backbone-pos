//! Restaurant lane behavior: table seating + transfer, course grouping, the one-draft-per-table
//! rule (service typed refusal + DB partial-unique backstop), and server-side order-level
//! discounts. POS-only — the tax port is the in-test fake; no cross-module seam is exercised here.
//! Requires DATABASE_URL (scratch DB with the module's migrations applied).

mod support;
use support::{d, pool, seed_profile, uq, TestTax};

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_pos::application::service::pos_write_service::{
    NewSale, NewSaleLine, NewSession, NewSyncSale, PosError, PosWriteService, SyncAction,
    SyncSaleLine, SyncTender,
};

fn at() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(2026, 7, 5).unwrap().and_hms_opt(9, 0, 0).unwrap()
}

async fn open(w: &PosWriteService, company: Uuid, prof: Uuid) -> Uuid {
    w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![],
    }).await.unwrap()
}

/// Seed one floor plan + one table on it; returns (floor_plan_id, table_id).
async fn seed_table(pool: &sqlx::PgPool, company: Uuid, name: &str) -> (Uuid, Uuid) {
    let floor = Uuid::new_v4();
    let table = Uuid::new_v4();
    sqlx::query("INSERT INTO pos.pos_floor_plans (id, company_id, name) VALUES ($1,$2,$3)")
        .bind(floor).bind(company).bind(name)
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO pos.pos_tables (id, company_id, pos_floor_plan_id, name, seats) VALUES ($1,$2,$3,$4,4)")
        .bind(table).bind(company).bind(floor).bind(name)
        .execute(pool).await.unwrap();
    (floor, table)
}

/// Seed an order-level discount master; returns its id.
async fn seed_discount(pool: &sqlx::PgPool, company: Uuid, pct: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO pos.pos_discounts (id, company_id, name, percentage) VALUES ($1,$2,'Happy Hour',$3)")
        .bind(id).bind(company).bind(d(pct))
        .execute(pool).await.unwrap();
    id
}

fn seated_sale(company: Uuid, prof: Uuid, session: Uuid, table: Option<Uuid>, discount: Option<Uuid>, receipt: &str, lines: Vec<NewSaleLine>) -> NewSale {
    NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session,
        branch_id: None, customer_id: None, pos_table_id: table, discount_id: discount,
        receipt_number: receipt.to_string(), posting_at: at(), lines,
    }
}

/// RT-1: a ring naming a table seats its draft there, and the lines' course grouping is persisted
/// and surfaced on the receipt read.
#[tokio::test]
async fn ring_seats_a_draft_and_carries_course() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::profile_at_rate(&pool, company, "0").await;
    let session = open(&w, company, prof).await;
    let (_floor, table) = seed_table(&pool, company, "T1").await;

    let id = w.ring_sale(seated_sale(company, prof, session, Some(table), None, &uq("REST-1"), vec![
        NewSaleLine { item_id: item, revenue_account_id: None, description: Some("Spring rolls".into()), course: Some(1), quantity: d("2"), unit_price: d("25000"), discount_amount: Decimal::ZERO },
        NewSaleLine { item_id: item, revenue_account_id: None, description: Some("Nasi goreng".into()), course: Some(2), quantity: d("1"), unit_price: d("50000"), discount_amount: Decimal::ZERO },
    ]), &tax).await.unwrap();

    let seated: Option<Uuid> = sqlx::query_scalar("SELECT pos_table_id FROM pos.pos_invoices WHERE id=$1")
        .bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(seated, Some(table));

    let courses: Vec<Option<i32>> = sqlx::query_scalar(
        "SELECT course FROM pos.pos_invoice_items WHERE pos_invoice_id=$1 AND (metadata->>'deleted_at') IS NULL ORDER BY course NULLS LAST",
    ).bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(courses, vec![Some(1), Some(2)]);

    let receipt = w.receipt(company, id).await.unwrap();
    assert_eq!(receipt.lines[0].course, Some(1));
    assert_eq!(receipt.lines[1].course, Some(2));
}

/// RT-2 (PSX-1): a second draft on an occupied table is refused LOUDLY — a typed 409 carrying both
/// the table and the occupying ticket, never a silent merge.
#[tokio::test]
async fn second_draft_on_occupied_table_refuses_loudly() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::zero_tax_profile(&pool, company).await;
    let session = open(&w, company, prof).await;
    let (_floor, table) = seed_table(&pool, company, "T2").await;

    let first = w.ring_sale(seated_sale(company, prof, session, Some(table), None, &uq("REST-2A"), vec![
        NewSaleLine { item_id: item, revenue_account_id: None, description: None, course: None, quantity: d("1"), unit_price: d("10000"), discount_amount: Decimal::ZERO },
    ]), &tax).await.unwrap();

    let e = w.ring_sale(seated_sale(company, prof, session, Some(table), None, &uq("REST-2B"), vec![
        NewSaleLine { item_id: item, revenue_account_id: None, description: None, course: None, quantity: d("1"), unit_price: d("10000"), discount_amount: Decimal::ZERO },
    ]), &tax).await.unwrap_err();
    match &e {
        PosError::TableOccupied { pos_table_id, draft_invoice_id } => {
            assert_eq!(*pos_table_id, table);
            assert_eq!(*draft_invoice_id, first, "the refusal names the occupying ticket");
        }
        other => panic!("expected TableOccupied, got {other:?}"),
    }
    assert_eq!(e.http_status(), 409, "an occupied table is a conflict, not a server error");
    assert_eq!(e.code(), "table_occupied");
}

/// RT-3 (PSX-1): the DB partial unique holds even when no service pre-check runs — two drafts
/// seated at one table cannot both exist, whatever wrote them.
#[tokio::test]
async fn db_partial_unique_backstops_one_draft_per_table() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let (_floor, table) = seed_table(&pool, company, "T3").await;

    let insert = |receipt: String| {
        sqlx::query(
            r#"INSERT INTO pos.pos_invoices (id, company_id, pos_profile_id, opening_entry_id, receipt_number, posting_at, net_total, tax_total, grand_total, rounding_adjustment, rounded_total, pos_table_id, status)
               VALUES ($1,$2,$3,$4,$5,$6,0,0,0,0,0,$7,'draft')"#,
        )
        .bind(Uuid::new_v4()).bind(company).bind(Uuid::new_v4()).bind(Uuid::new_v4())
        .bind(receipt).bind(at()).bind(table)
    };
    let r1 = insert(uq("REST-3A")).execute(&pool).await;
    assert!(r1.is_ok(), "first draft seats fine: {r1:?}");
    let r2 = insert(uq("REST-3B")).execute(&pool).await;
    let err = r2.expect_err("the second direct insert must be refused by the partial unique");
    let db = err.as_database_error().expect("unique violation");
    assert!(db.is_unique_violation(), "expected a unique violation, got: {db}");
    assert!(db.constraint().unwrap().contains("pos_table_id"),
        "the refusing constraint names the table column: {:?}", db.constraint());
}

/// RT-4: a table nobody seated at does not exist — typed 404, not a silent unseated ring.
#[tokio::test]
async fn unknown_table_refuses() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::zero_tax_profile(&pool, company).await;
    let session = open(&w, company, prof).await;

    let ghost = Uuid::new_v4();
    let e = w.ring_sale(seated_sale(company, prof, session, Some(ghost), None, &uq("REST-4"), vec![
        NewSaleLine { item_id: item, revenue_account_id: None, description: None, course: None, quantity: d("1"), unit_price: d("10000"), discount_amount: Decimal::ZERO },
    ]), &tax).await.unwrap_err();
    assert!(matches!(e, PosError::TableNotFound(t) if t == ghost));
    assert_eq!(e.http_status(), 404);
}

/// RT-5: a table TRANSFER via the offline replay moves the draft — the old table frees up (a new
/// draft may seat there) and the new table now holds exactly the moved ticket.
#[tokio::test]
async fn sync_transfer_moves_the_draft_and_frees_the_old_table() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::zero_tax_profile(&pool, company).await;
    let session = open(&w, company, prof).await;
    let (_f1, table_a) = seed_table(&pool, company, "T5A").await;
    let (_f2, table_b) = seed_table(&pool, company, "T5B").await;

    let client = Uuid::new_v4();
    let mk_sync = |table: Option<Uuid>, client: Uuid| NewSyncSale {
        company_id: company, client_uuid: client, pos_profile_id: prof, opening_entry_id: session,
        rescue_opening_entry_id: None, branch_id: None, customer_id: None,
        pos_table_id: table, discount_id: None, posting_at: at(),
        lines: vec![SyncSaleLine {
            client_uuid: Uuid::new_v4(), item_id: item, revenue_account_id: None, description: None,
            course: Some(1), quantity: d("1"), unit_price: d("20000"), discount_amount: Decimal::ZERO,
        }],
        tenders: vec![SyncTender { client_uuid: Uuid::new_v4(), method: "cash".into(), amount: d("20000"), reference_no: None }],
        refund_of_client_uuid: None, manager: None, source_ip: None,
    };

    let out = w.sync_from_ui(mk_sync(Some(table_a), client), &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap();
    assert_eq!(out.action, SyncAction::Created);

    // Transfer: same client uuid, new table.
    let out2 = w.sync_from_ui(mk_sync(Some(table_b), client), &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap();
    assert_eq!(out2.action, SyncAction::Updated);
    assert_eq!(out2.pos_invoice_id, out.pos_invoice_id, "a transfer is an update, never a second ticket");

    let seated: Option<Uuid> = sqlx::query_scalar("SELECT pos_table_id FROM pos.pos_invoices WHERE id=$1")
        .bind(out.pos_invoice_id).fetch_one(&pool).await.unwrap();
    assert_eq!(seated, Some(table_b));

    // The old table is free: a fresh draft seats there without refusal.
    let other_client = Uuid::new_v4();
    w.sync_from_ui(mk_sync(Some(table_a), other_client), &tax, &support::StubBilling::default(), &support::StubPayment)
        .await
        .expect("the vacated table accepts a new draft");
}

/// RT-6 (PSX-1 on the replay path): transferring onto a table another draft holds is a typed 409
/// naming the occupant.
#[tokio::test]
async fn sync_transfer_onto_occupied_table_refuses() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::zero_tax_profile(&pool, company).await;
    let session = open(&w, company, prof).await;
    let (_f1, table_a) = seed_table(&pool, company, "T6A").await;
    let (_f2, table_b) = seed_table(&pool, company, "T6B").await;

    let mk_sync = |table: Option<Uuid>, client: Uuid| NewSyncSale {
        company_id: company, client_uuid: client, pos_profile_id: prof, opening_entry_id: session,
        rescue_opening_entry_id: None, branch_id: None, customer_id: None,
        pos_table_id: table, discount_id: None, posting_at: at(),
        lines: vec![SyncSaleLine {
            client_uuid: Uuid::new_v4(), item_id: item, revenue_account_id: None, description: None,
            course: None, quantity: d("1"), unit_price: d("10000"), discount_amount: Decimal::ZERO,
        }],
        tenders: vec![],
        refund_of_client_uuid: None, manager: None, source_ip: None,
    };

    let mover = Uuid::new_v4();
    let squatter = Uuid::new_v4();
    w.sync_from_ui(mk_sync(Some(table_a), mover), &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap();
    let squatter_id = w.sync_from_ui(mk_sync(Some(table_b), squatter), &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap().pos_invoice_id;

    let e = w.sync_from_ui(mk_sync(Some(table_b), mover), &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap_err();
    match &e {
        PosError::TableOccupied { pos_table_id, draft_invoice_id } => {
            assert_eq!(*pos_table_id, table_b);
            assert_eq!(*draft_invoice_id, squatter_id);
        }
        other => panic!("expected TableOccupied, got {other:?}"),
    }
    assert_eq!(e.http_status(), 409);
}

/// RT-7 (order discount): the percentage comes from the tenant's MASTER row, folds pro-rata into
/// the line discounts BEFORE the tax compute (tax sees post-discount nets), and the totals reflect
/// it end to end.
#[tokio::test]
async fn order_discount_folds_server_side_from_the_master() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::profile_at_rate(&pool, company, "0.11").await;
    let session = open(&w, company, prof).await;
    let discount = seed_discount(&pool, company, "0.1000").await;

    let id = w.ring_sale(seated_sale(company, prof, session, None, Some(discount), &uq("REST-7"), vec![
        NewSaleLine { item_id: item, revenue_account_id: None, description: None, course: None, quantity: d("2"), unit_price: d("25000"), discount_amount: Decimal::ZERO },
        NewSaleLine { item_id: item, revenue_account_id: None, description: None, course: None, quantity: d("1"), unit_price: d("50000"), discount_amount: Decimal::ZERO },
    ]), &tax).await.unwrap();

    // Gross 100,000; 10% order discount = 10,000 folded 5,000/5,000 by line gross.
    // Net 90,000; tax 11% on the DISCOUNTED nets = 9,900; grand 99,900.
    let (net, tax_total, grand): (Decimal, Decimal, Decimal) = sqlx::query_as(
        "SELECT net_total, tax_total, grand_total FROM pos.pos_invoices WHERE id=$1",
    ).bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(net, d("90000.00"));
    assert_eq!(tax_total, d("9900.00"));
    assert_eq!(grand, d("99900.00"));

    let discounts: Vec<Decimal> = sqlx::query_scalar(
        "SELECT discount_amount FROM pos.pos_invoice_items WHERE pos_invoice_id=$1 AND (metadata->>'deleted_at') IS NULL",
    ).bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(discounts, vec![d("5000.00"), d("5000.00")]);
}

/// RT-8 (tenant fence on the discount master): a discount id seeded under ANOTHER company is
/// unknown to this tenant — typed 404, never a cross-tenant rate application.
#[tokio::test]
async fn discount_master_is_tenant_scoped() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::zero_tax_profile(&pool, company).await;
    let session = open(&w, company, prof).await;

    let foreign = seed_discount(&pool, Uuid::new_v4(), "0.5").await;
    let e = w.ring_sale(seated_sale(company, prof, session, None, Some(foreign), &uq("REST-8"), vec![
        NewSaleLine { item_id: item, revenue_account_id: None, description: None, course: None, quantity: d("1"), unit_price: d("10000"), discount_amount: Decimal::ZERO },
    ]), &tax).await.unwrap_err();
    assert!(matches!(e, PosError::DiscountNotFound(d) if d == foreign));
    assert_eq!(e.http_status(), 404);
}

/// RT-9: a register with discounts disabled (`allow_discount = false`) refuses the order discount —
/// the master existing is not enough.
#[tokio::test]
async fn register_with_discounts_off_refuses() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let template = Uuid::new_v4();
    let prof = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, tax_template_ids, allow_discount, status)
           VALUES ($1,$2,'No Discount Register','IDR',$3,false,'active')"#,
    )
    .bind(prof).bind(company).bind(serde_json::json!([template.to_string()]))
    .execute(&pool).await.unwrap();
    let tax = TestTax::with_rate(template, "0");
    let session = open(&w, company, prof).await;
    let discount = seed_discount(&pool, company, "0.1000").await;

    let e = w.ring_sale(seated_sale(company, prof, session, None, Some(discount), &uq("REST-9"), vec![
        NewSaleLine { item_id: item, revenue_account_id: None, description: None, course: None, quantity: d("1"), unit_price: d("10000"), discount_amount: Decimal::ZERO },
    ]), &tax).await.unwrap_err();
    assert!(matches!(e, PosError::DiscountNotAllowed));
    assert_eq!(e.http_status(), 422);
}

/// RT-10: the offline replay prices the SAME order discount as the online ring (the shared compute
/// core) — an offline ticket and its online twin can never disagree.
#[tokio::test]
async fn sync_replay_applies_the_same_order_discount() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::profile_at_rate(&pool, company, "0.11").await;
    let session = open(&w, company, prof).await;
    let discount = seed_discount(&pool, company, "0.1000").await;

    let out = w.sync_from_ui(NewSyncSale {
        company_id: company, client_uuid: Uuid::new_v4(), pos_profile_id: prof, opening_entry_id: session,
        rescue_opening_entry_id: None, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: Some(discount), posting_at: at(),
        lines: vec![SyncSaleLine {
            client_uuid: Uuid::new_v4(), item_id: item, revenue_account_id: None, description: None,
            course: None, quantity: d("2"), unit_price: d("25000"), discount_amount: Decimal::ZERO,
        }],
        tenders: vec![],
        refund_of_client_uuid: None, manager: None, source_ip: None,
    }, &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap();

    assert_eq!(out.totals.net_total, d("45000.00"));
    assert_eq!(out.totals.tax_total, d("4950.00"));
    assert_eq!(out.totals.grand_total, d("49950.00"));
}
