//! The offline replay verb (`sync_from_ui`): identity by `client_uuid`, totals ALWAYS recomputed
//! server-side, partner/session validated (rescue-or-refuse), refund lineage single-parent and
//! privileged. POS-only — the refund replay drives stub billing/payment ports (the real reversal
//! seam is proven in `retail_sale_seam.rs`). Requires DATABASE_URL (:5433/backbone_pos).

mod support;
use support::{at, d, manager_with_pin, pool, seed_profile, uq, RecordingVariance, StubBilling, StubPayment, TestTax};

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_pos::application::service::pos_write_service::{
    ManagerAuth, NewClose, NewSession, NewSyncSale, PosError, PosWriteService, SyncAction,
};

fn sync_line(client: Uuid, item: Uuid, qty: &str, price: &str) -> backbone_pos::application::service::pos_write_service::SyncSaleLine {
    backbone_pos::application::service::pos_write_service::SyncSaleLine {
        client_uuid: client, item_id: item, revenue_account_id: None, description: None,
        course: None, quantity: d(qty), unit_price: d(price), discount_amount: Decimal::ZERO,
    }
}
fn tender(client: Uuid, method: &str, amount: &str) -> backbone_pos::application::service::pos_write_service::SyncTender {
    backbone_pos::application::service::pos_write_service::SyncTender {
        client_uuid: client, method: method.into(), amount: d(amount), reference_no: None,
    }
}
fn sync(company: Uuid, client: Uuid, prof: Uuid, session: Uuid, lines: Vec<backbone_pos::application::service::pos_write_service::SyncSaleLine>, tenders: Vec<backbone_pos::application::service::pos_write_service::SyncTender>) -> NewSyncSale {
    NewSyncSale {
        company_id: company, client_uuid: client, pos_profile_id: prof, opening_entry_id: session,
        rescue_opening_entry_id: None, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None, posting_at: at(),
        lines, tenders, refund_of_client_uuid: None, manager: None, source_ip: None,
    }
}

/// A register with templates + the GL accounts + walk-in customer recognition needs (tests that
/// finalize tickets use this).
async fn full_profile(pool: &sqlx::PgPool, company: Uuid, rate: &str) -> (Uuid, TestTax) {
    let template = Uuid::new_v4();
    let id = Uuid::new_v4();
    sqlx::query(r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, tax_template_ids, receivable_account_id, income_account_id, cash_account_id, default_customer_id, tax_account_id, allow_discount, status)
        VALUES ($1,$2,'Register 1','IDR',$3,$4,$5,$6,$7,$8,true,'active')"#)
        .bind(id).bind(company).bind(serde_json::json!([template.to_string()]))
        .bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4())
        .execute(pool).await.unwrap();
    (id, TestTax::with_rate(template, rate))
}

async fn open(w: &PosWriteService, company: Uuid, prof: Uuid) -> Uuid {
    w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![],
    }).await.unwrap()
}

/// SY-1: a first replay CREATES the ticket with server-derived money — the payload's lines + tenders
/// are inputs only; a fully-tendered replay also fires the recognition trigger.
#[tokio::test]
async fn first_replay_creates_with_server_totals() {
    let pool = pool().await;
    let rec = support::Recorder::default();
    let w = PosWriteService::with_sink(pool.clone(), std::sync::Arc::new(rec.clone()));
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = full_profile(&pool, company, "0.11").await;
    let session = open(&w, company, prof).await;

    let client = Uuid::new_v4();
    let out = w.sync_from_ui(
        sync(company, client, prof, session,
            vec![sync_line(Uuid::new_v4(), item, "2", "50000")],
            vec![tender(Uuid::new_v4(), "cash", "111000")]),
        &tax, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap();
    assert_eq!(out.action, SyncAction::Created);
    // Server-derived: net 100,000, tax 11,000, grand 111,000, paid 111,000, change 0.
    assert_eq!(out.totals.net_total, d("100000.00"));
    assert_eq!(out.totals.tax_total, d("11000.00"));
    assert_eq!(out.totals.rounded_total, d("111000.00"));
    assert_eq!(out.totals.paid_total, d("111000.00"));
    assert_eq!(out.totals.change_due, d("0.00"));

    // The ticket carries the sync identity; a fully-tendered replay triggered recognition.
    let (cu, st, rn): (Option<Uuid>, String, String) = sqlx::query_as(
        "SELECT client_uuid, status::text, receipt_number FROM pos.pos_invoices WHERE id=$1",
    ).bind(out.pos_invoice_id).fetch_one(&pool).await.unwrap();
    assert_eq!(cu, Some(client));
    assert_eq!(st, "draft");
    assert!(rn.starts_with("SYNC-"), "the server mints the receipt number from the sync identity");
    let fired = rec.events.lock().unwrap().iter().any(|e| matches!(
        e, backbone_pos::application::service::pos_events::PosEvent::PosTenderCompleted(t) if t.pos_invoice_id == out.pos_invoice_id));
    assert!(fired, "a fully-tendered replay fires the recognition trigger");
}

/// SY-2: a replay of the SAME uuid while the ticket is still draft REWRITES it — new lines/tenders
/// replace the old (soft-deleted), totals recomputed. Not a second ticket.
#[tokio::test]
async fn replay_updates_the_draft_it_names() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = full_profile(&pool, company, "0").await;
    let session = open(&w, company, prof).await;

    let client = Uuid::new_v4();
    let first = w.sync_from_ui(
        sync(company, client, prof, session,
            vec![sync_line(Uuid::new_v4(), item, "2", "50000"), sync_line(Uuid::new_v4(), item, "1", "20000")],
            vec![tender(Uuid::new_v4(), "cash", "120000")]),
        &tax, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap();
    assert_eq!(first.action, SyncAction::Created);
    assert_eq!(first.totals.net_total, d("120000.00"));

    // The cashier removed the 20,000 line and paid by card instead.
    let second = w.sync_from_ui(
        sync(company, client, prof, session,
            vec![sync_line(Uuid::new_v4(), item, "2", "50000")],
            vec![tender(Uuid::new_v4(), "card", "100000")]),
        &tax, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap();
    assert_eq!(second.pos_invoice_id, first.pos_invoice_id, "the same ticket, not a second one");
    assert_eq!(second.action, SyncAction::Updated);
    assert_eq!(second.totals.net_total, d("100000.00"));
    assert_eq!(second.totals.paid_total, d("100000.00"));

    // Exactly one live line + one live tender; the superseded ones are soft-deleted (audit kept).
    let live_lines: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pos.pos_invoice_items WHERE pos_invoice_id=$1 AND metadata->>'deleted_at' IS NULL")
        .bind(first.pos_invoice_id).fetch_one(&pool).await.unwrap();
    let dead_lines: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pos.pos_invoice_items WHERE pos_invoice_id=$1 AND metadata->>'deleted_at' IS NOT NULL")
        .bind(first.pos_invoice_id).fetch_one(&pool).await.unwrap();
    let live_tenders: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pos.pos_payments WHERE pos_invoice_id=$1 AND metadata->>'deleted_at' IS NULL")
        .bind(first.pos_invoice_id).fetch_one(&pool).await.unwrap();
    assert_eq!((live_lines, dead_lines, live_tenders), (1, 2, 1));
    let (st, paid, method): (String, Decimal, String) = sqlx::query_as(
        "SELECT i.status::text, i.paid_total, p.payment_method::text FROM pos.pos_invoices i JOIN pos.pos_payments p ON p.pos_invoice_id=i.id AND p.metadata->>'deleted_at' IS NULL WHERE i.id=$1")
        .bind(first.pos_invoice_id).fetch_one(&pool).await.unwrap();
    assert_eq!((st.as_str(), paid, method.as_str()), ("draft", d("100000.00"), "card"));
    let tickets: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pos.pos_invoices WHERE client_uuid=$1")
        .bind(client).fetch_one(&pool).await.unwrap();
    assert_eq!(tickets, 1, "identity is the uuid — replays never double-ring");
}

/// SY-3: a replay of a FINALIZED ticket changes nothing — the server's state and money win.
#[tokio::test]
async fn replay_of_a_finalized_ticket_is_inert() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = full_profile(&pool, company, "0.11").await;
    let session = open(&w, company, prof).await;

    let client = Uuid::new_v4();
    let first = w.sync_from_ui(
        sync(company, client, prof, session,
            vec![sync_line(Uuid::new_v4(), item, "1", "100000")],
            vec![tender(Uuid::new_v4(), "cash", "111000")]),
        &tax, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap();
    w.recognize_sale(first.pos_invoice_id, &support::StubBilling::default(), &support::StubPayment, None).await.unwrap();

    // A late replay (changed lines, whatever tenders) is refused-as-replay: nothing rewritten.
    let replay = w.sync_from_ui(
        sync(company, client, prof, session,
            vec![sync_line(Uuid::new_v4(), item, "5", "100000")],
            vec![tender(Uuid::new_v4(), "cash", "555000")]),
        &tax, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap();
    assert_eq!(replay.action, SyncAction::ReplayFinalized);
    assert_eq!(replay.pos_invoice_id, first.pos_invoice_id);
    assert_eq!(replay.totals.net_total, d("100000.00"), "the SERVER's persisted money is the answer");
    let (st, net): (String, Decimal) = sqlx::query_as("SELECT status::text, net_total FROM pos.pos_invoices WHERE id=$1")
        .bind(first.pos_invoice_id).fetch_one(&pool).await.unwrap();
    assert_eq!((st.as_str(), net), ("paid", d("100000.00")));
}

/// SY-4: rescue-or-refuse. A replay under a session that has since closed is refused with the typed
/// error; naming an OPEN session of the SAME register rescues the ticket onto that shift.
#[tokio::test]
async fn closed_session_is_rescue_or_refuse() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = full_profile(&pool, company, "0").await;
    let manager = manager_with_pin(&w, company, "4321").await;
    let variance = RecordingVariance::default();

    // Shift 1 opens and closes with no tickets; shift 2 is open on the same register.
    let s1 = open(&w, company, prof).await;
    w.close_session(NewClose {
        company_id: company, opening_entry_id: s1, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![], manager: manager.clone(), source_ip: None,
    }, &variance).await.unwrap();
    let s2 = open(&w, company, prof).await;

    let client = Uuid::new_v4();
    let mut req = sync(company, client, prof, s1,
        vec![sync_line(Uuid::new_v4(), item, "1", "50000")],
        vec![tender(Uuid::new_v4(), "cash", "50000")]);
    // No rescue named → typed refusal.
    let e = w.sync_from_ui(req.clone(), &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap_err();
    assert!(matches!(e, PosError::SessionClosedRescueRequired(_)));

    // The same replay naming shift 2 as the rescue lands there.
    req.rescue_opening_entry_id = Some(s2);
    let out = w.sync_from_ui(req, &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap();
    assert_eq!(out.action, SyncAction::Created);
    let landed: Uuid = sqlx::query_scalar("SELECT opening_entry_id FROM pos.pos_invoices WHERE id=$1")
        .bind(out.pos_invoice_id).fetch_one(&pool).await.unwrap();
    assert_eq!(landed, s2, "the rescued ticket rides the open shift");
}

/// SY-5: a rescue onto ANOTHER register's open session is refused — that would move money between
/// tills. Also: a plain replay naming another register's session while its own is still open is a
/// mismatch, not a rescue.
#[tokio::test]
async fn rescue_must_stay_on_the_tickets_register() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof_a, tax) = full_profile(&pool, company, "0").await;
    let (prof_b, _) = support::profile_at_rate(&pool, company, "0").await;
    let manager = manager_with_pin(&w, company, "4321").await;
    let variance = RecordingVariance::default();

    let s1 = open(&w, company, prof_a).await;
    w.close_session(NewClose {
        company_id: company, opening_entry_id: s1, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![], manager, source_ip: None,
    }, &variance).await.unwrap();
    let other_register_session = open(&w, company, prof_b).await;

    let client = Uuid::new_v4();
    let req = sync(company, client, prof_a, s1,
        vec![sync_line(Uuid::new_v4(), item, "1", "50000")],
        vec![tender(Uuid::new_v4(), "cash", "50000")]);
    let mut req = req;
    req.rescue_opening_entry_id = Some(other_register_session);
    let e = w.sync_from_ui(req, &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap_err();
    assert!(matches!(e, PosError::SessionRegisterMismatch), "cross-register rescue refused, got {e:?}");
}

/// SY-5b: the plain-open path owes the same register pairing the rescue arms enforce — a replay
/// naming register A's config against register B's OPEN session refuses, even with no rescue in
/// play. (Before this check the plain-open arm returned the session unchecked.)
#[tokio::test]
async fn plain_open_replay_must_stay_on_its_register() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof_a, tax) = full_profile(&pool, company, "0").await;
    let (prof_b, _) = support::profile_at_rate(&pool, company, "0").await;

    // Register B's session is open; the payload claims register A's config.
    let session_b = open(&w, company, prof_b).await;
    let e = w.sync_from_ui(
        sync(company, Uuid::new_v4(), prof_a, session_b,
            vec![sync_line(Uuid::new_v4(), item, "1", "50000")],
            vec![tender(Uuid::new_v4(), "cash", "50000")]),
        &tax, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap_err();
    assert!(matches!(e, PosError::SessionRegisterMismatch), "cross-register plain-open replay refused, got {e:?}");
    let tickets: i64 = sqlx::query_scalar("SELECT count(*) FROM pos.pos_invoices WHERE company_id=$1")
        .bind(company).fetch_one(&pool).await.unwrap();
    assert_eq!(tickets, 0, "nothing was written");
}

/// SY-6: a refund replay is PRIVILEGED — manager verified — and single-parent: it drives the same
/// idempotent full-return flow `return_sale` uses; a replay of the refund is inert.
#[tokio::test]
async fn refund_replay_is_privileged_and_idempotent() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = full_profile(&pool, company, "0").await;
    let session = open(&w, company, prof).await;
    let manager = manager_with_pin(&w, company, "4321").await;

    // A recognized parent sale, created by its own replay.
    let parent_uuid = Uuid::new_v4();
    let parent = w.sync_from_ui(
        sync(company, parent_uuid, prof, session,
            vec![sync_line(Uuid::new_v4(), item, "1", "100000")],
            vec![tender(Uuid::new_v4(), "cash", "100000")]),
        &tax, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap();
    w.recognize_sale(parent.pos_invoice_id, &support::StubBilling::default(), &support::StubPayment, None).await.unwrap();

    // The refund replay: new uuid + refund_of_client_uuid naming the parent.
    let mut req = sync(company, Uuid::new_v4(), prof, session, vec![], vec![]);
    req.refund_of_client_uuid = Some(parent_uuid);

    // Without the manager proof → refused.
    let e = w.sync_from_ui(req.clone(), &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap_err();
    assert!(matches!(e, PosError::ManagerAuthRequired));
    // With a WRONG PIN → refused, nothing returned.
    req.manager = Some(ManagerAuth { employee_party_id: manager.employee_party_id, pin: "9999".into() });
    let e = w.sync_from_ui(req.clone(), &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap_err();
    assert!(matches!(e, PosError::PinInvalid));
    // With the right PIN → the parent is returned, a return ticket recorded.
    req.manager = Some(manager.clone());
    let out = w.sync_from_ui(req.clone(), &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap();
    assert_eq!(out.action, SyncAction::Created);
    let (pst, is_ret, against): (String, bool, Option<Uuid>) = sqlx::query_as(
        "SELECT status::text, is_return, return_against FROM pos.pos_invoices WHERE id=$1")
        .bind(parent.pos_invoice_id).fetch_one(&pool).await.unwrap();
    assert_eq!((pst.as_str(), is_ret, against), ("returned", false, None));
    let (rst, rret, ragainst): (String, bool, Option<Uuid>) = sqlx::query_as(
        "SELECT status::text, is_return, return_against FROM pos.pos_invoices WHERE id=$1")
        .bind(out.pos_invoice_id).fetch_one(&pool).await.unwrap();
    assert_eq!(rst, "returned");
    assert!(rret);
    assert_eq!(ragainst, Some(parent.pos_invoice_id));

    // Replaying the refund (same manager) is inert — the parent is already returned.
    let replay = w.sync_from_ui(req, &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap();
    assert_eq!(replay.action, SyncAction::ReplayFinalized);
    assert_eq!(replay.pos_invoice_id, out.pos_invoice_id, "the same return ticket");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pos.pos_invoices WHERE return_against=$1 AND is_return")
        .bind(parent.pos_invoice_id).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1, "exactly one return ticket per parent");
}

/// SY-7: refund guards — an unknown parent uuid refuses; a parent that is still DRAFT is not
/// returnable yet.
#[tokio::test]
async fn refund_parent_must_exist_and_be_finalized() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = full_profile(&pool, company, "0").await;
    let session = open(&w, company, prof).await;
    let manager = manager_with_pin(&w, company, "4321").await;

    let mut req = sync(company, Uuid::new_v4(), prof, session, vec![], vec![]);
    req.refund_of_client_uuid = Some(Uuid::new_v4());
    req.manager = Some(manager.clone());
    let e = w.sync_from_ui(req, &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap_err();
    assert!(matches!(e, PosError::RefundParentNotFound(_)));

    // A DRAFT parent: recognition must complete before any refund.
    let parent_uuid = Uuid::new_v4();
    w.sync_from_ui(
        sync(company, parent_uuid, prof, session,
            vec![sync_line(Uuid::new_v4(), item, "1", "100000")],
            vec![tender(Uuid::new_v4(), "cash", "100000")]),
        &tax, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap();
    let mut req = sync(company, Uuid::new_v4(), prof, session, vec![], vec![]);
    req.refund_of_client_uuid = Some(parent_uuid);
    req.manager = Some(manager);
    let e = w.sync_from_ui(req, &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap_err();
    assert!(matches!(e, PosError::NotReturnable(_)));
}

/// SY-8: lineage + partner fences on the UPDATE path — a draft cannot morph into a refund, a refund
/// ticket cannot be re-pointed, and a replay cannot re-assign the customer.
#[tokio::test]
async fn update_path_fences_lineage_and_partner() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = full_profile(&pool, company, "0").await;
    let session = open(&w, company, prof).await;
    let manager = manager_with_pin(&w, company, "4321").await;

    let client = Uuid::new_v4();
    w.sync_from_ui(
        sync(company, client, prof, session,
            vec![sync_line(Uuid::new_v4(), item, "1", "50000")],
            vec![tender(Uuid::new_v4(), "cash", "50000")]),
        &tax, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap();

    // A plain replay carrying refund_of is a lineage conflict — refunds ride the dedicated path only.
    let mut req = sync(company, client, prof, session, vec![], vec![]);
    req.refund_of_client_uuid = Some(Uuid::new_v4());
    req.manager = Some(manager);
    let e = w.sync_from_ui(req, &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap_err();
    assert!(matches!(e, PosError::RefundLineageConflict));

    // A replay naming a DIFFERENT customer cannot re-assign the ticket's partner.
    let mut req = sync(company, client, prof, session,
        vec![sync_line(Uuid::new_v4(), item, "1", "50000")],
        vec![tender(Uuid::new_v4(), "cash", "50000")]);
    req.customer_id = Some(Uuid::new_v4());
    let e = w.sync_from_ui(req, &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap_err();
    assert!(matches!(e, PosError::SyncPartnerMismatch));
}

/// SY-9: tender methods are validated against the live enum — a typo'd method is a typed 422 before
/// any write, not a DB cast failure mid-transaction.
#[tokio::test]
async fn unknown_tender_method_is_a_typed_refusal() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = full_profile(&pool, company, "0").await;
    let session = open(&w, company, prof).await;

    let req = sync(company, Uuid::new_v4(), prof, session,
        vec![sync_line(Uuid::new_v4(), item, "1", "1000")],
        vec![tender(Uuid::new_v4(), "crypto", "1000")]);
    let e = w.sync_from_ui(req, &tax, &support::StubBilling::default(), &support::StubPayment).await.unwrap_err();
    match e {
        PosError::InvalidTenderMethod(m) => assert_eq!(m, "crypto"),
        other => panic!("expected InvalidTenderMethod, got {other:?}"),
    }
}

/// SY-10: tenant scoping — the identity lookup rides the company fence, so another company's ticket
/// carrying the same uuid is invisible (the replay creates its own, and never touches the other's).
#[tokio::test]
async fn identity_lookup_is_tenant_scoped() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company_a, company_b, item) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (prof_a, tax) = full_profile(&pool, company_a, "0").await;
    let (prof_b, tax_b) = support::profile_at_rate(&pool, company_b, "0").await;
    let s_a = open(&w, company_a, prof_a).await;
    let s_b = open(&w, company_b, prof_b).await;

    let client = Uuid::new_v4();
    let a = w.sync_from_ui(
        sync(company_a, client, prof_a, s_a,
            vec![sync_line(Uuid::new_v4(), item, "1", "100000")],
            vec![tender(Uuid::new_v4(), "cash", "100000")]),
        &tax, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap();
    let b = w.sync_from_ui(
        sync(company_b, client, prof_b, s_b,
            vec![sync_line(Uuid::new_v4(), item, "1", "1000")],
            vec![tender(Uuid::new_v4(), "cash", "1000")]),
        &tax_b, &support::StubBilling::default(), &support::StubPayment,
    ).await.unwrap();
    assert_ne!(a.pos_invoice_id, b.pos_invoice_id, "the uuid namespaces per company");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pos.pos_invoices WHERE client_uuid=$1 AND company_id=$2")
        .bind(client).bind(company_a).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1);
}
