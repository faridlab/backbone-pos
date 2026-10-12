//! Tenders, replays and recognition under concurrency and retry: parallel tenders both count, change
//! only comes out of cash, a replay of a paid-up ticket triggers nothing, two deliveries of the same
//! trigger raise one invoice, and a stock relief refused after the ticket was marked paid is retried
//! rather than skipped. POS-only — billing, payment and inventory are fakes (the real seam is
//! `retail_sale_seam.rs`). Requires DATABASE_URL (:5433/backbone_pos).
//!
//! PTR-1..PTR-6, one per case below.

mod support;
use support::{at, d, pool, StubBilling, StubPayment, TestTax};

use std::sync::{Arc, Mutex};

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_pos::application::service::pos_events::PosEvent;
use backbone_pos::application::service::pos_ports::{
    BillingPort, CreditNoteRequest, InventoryPort, InvoiceAck, PosRejected, ReversalAck,
    SaleInvoiceRequest, StockIssueAck, StockIssueRequest,
};
use backbone_pos::application::service::pos_write_service::{
    NewSession, NewSyncSale, PosError, PosWriteService, SyncSaleLine, SyncTender,
};

fn line(item: Uuid, qty: &str, price: &str) -> SyncSaleLine {
    SyncSaleLine {
        client_uuid: Uuid::new_v4(), item_id: item, revenue_account_id: None, description: None,
        course: None, quantity: d(qty), unit_price: d(price), discount_amount: Decimal::ZERO,
    }
}
fn tender(method: &str, amount: &str) -> SyncTender {
    SyncTender { client_uuid: Uuid::new_v4(), method: method.into(), amount: d(amount), reference_no: None }
}
fn sync(client: Uuid, prof: Uuid, session: Uuid, lines: Vec<SyncSaleLine>, tenders: Vec<SyncTender>) -> NewSyncSale {
    NewSyncSale {
        client_uuid: client, pos_profile_id: prof, opening_entry_id: session,
        rescue_opening_entry_id: None, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None, posting_at: at(),
        lines, tenders, refund_of_client_uuid: None, manager: None, source_ip: None,
    }
}

/// A zero-tax register with every account recognition needs, and, when `stock` is set, the stock
/// seats (warehouse, COGS and inventory accounts) that make it relieve stock.
async fn register(pool: &sqlx::PgPool, stock: bool) -> (Uuid, TestTax) {
    let template = Uuid::new_v4();
    let id = Uuid::new_v4();
    let seat = |on: bool| if on { Some(Uuid::new_v4()) } else { None };
    sqlx::query(r#"INSERT INTO pos.pos_profiles (id, name, currency, tax_template_ids, receivable_account_id, income_account_id, cash_account_id, default_customer_id, tax_account_id, warehouse_id, cogs_account_id, inventory_account_id, allow_discount, status)
        VALUES ($1,'Register 1','IDR',$2,$3,$4,$5,$6,$7,$8,$9,$10,true,'active')"#)
        .bind(id).bind(serde_json::json!([template.to_string()]))
        .bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4())
        .bind(seat(stock)).bind(seat(stock)).bind(seat(stock))
        .execute(pool).await.unwrap();
    (id, TestTax::with_rate(template, "0"))
}

async fn open(w: &PosWriteService, prof: Uuid) -> Uuid {
    w.open_session(NewSession {
        pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![],
    }).await.unwrap()
}

/// A draft ticket of 100,000 with the given tenders, created by a first replay.
async fn draft(w: &PosWriteService, prof: Uuid, tax: &TestTax, tenders: Vec<SyncTender>) -> Uuid {
    let session = open(w, prof).await;
    w.sync_from_ui(
        sync(Uuid::new_v4(), prof, session, vec![line(Uuid::new_v4(), "1", "100000")], tenders),
        tax, &StubBilling::default(), &StubPayment,
    ).await.unwrap().pos_invoice_id
}

fn tender_completions(events: &Arc<Mutex<Vec<PosEvent>>>, id: Uuid) -> usize {
    events.lock().unwrap().iter()
        .filter(|e| matches!(e, PosEvent::PosTenderCompleted(t) if t.pos_invoice_id == id))
        .count()
}

/// PTR-1: parallel tenders on one ticket all count. Each takes the header lock, so each re-sums
/// with the earlier ones in it, and exactly one of them is the crossing that triggers recognition.
#[tokio::test]
async fn parallel_tenders_all_count() {
    let pool = pool().await;
    let rec = support::Recorder::default();
    let w = Arc::new(PosWriteService::with_sink(pool.clone(), Arc::new(rec.clone())));
    let (prof, tax) = register(&pool, false).await;
    let id = draft(&w, prof, &tax, vec![]).await;

    let tenders: Vec<_> = (0..10)
        .map(|_| {
            let w = w.clone();
            tokio::spawn(async move { w.add_tender(id, "cash", d("10000"), None).await })
        })
        .collect();
    for t in tenders {
        t.await.unwrap().unwrap();
    }
    let (paid, change): (Decimal, Decimal) =
        sqlx::query_as("SELECT paid_total, change_due FROM pos.pos_invoices WHERE id=$1")
            .bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(paid, d("100000.00"), "every tender is in the header's paid total");
    assert_eq!(change, d("0.00"));
    assert_eq!(tender_completions(&rec.events, id), 1, "one tender crossed full payment");
}

/// PTR-2: change only comes out of cash. A card may cover at most what is still due; cash may
/// overpay and takes change.
#[tokio::test]
async fn only_cash_may_overpay() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (prof, tax) = register(&pool, false).await;
    let id = draft(&w, prof, &tax, vec![]).await;

    let e = w.add_tender(id, "card", d("100000.01"), None).await.unwrap_err();
    assert!(matches!(e, PosError::NonCashOverpayment { .. }), "got {e:?}");
    w.add_tender(id, "card", d("60000"), None).await.unwrap();
    let e = w.add_tender(id, "qris", d("40000.01"), None).await.unwrap_err();
    assert!(matches!(e, PosError::NonCashOverpayment { .. }), "got {e:?}");
    let out = w.add_tender(id, "cash", d("50000"), None).await.unwrap();
    assert_eq!(out.paid_total, d("110000.00"));
    assert_eq!(out.change_due, d("10000.00"), "the change is cash the drawer took");
}

/// PTR-3: the same rule on the offline replay: tenders other than cash may cover at most the
/// ticket, and nothing is written when they cover more.
#[tokio::test]
async fn a_replay_cannot_overpay_by_card() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (prof, tax) = register(&pool, false).await;
    let session = open(&w, prof).await;
    let client = Uuid::new_v4();
    let e = w.sync_from_ui(
        sync(client, prof, session, vec![line(Uuid::new_v4(), "1", "100000")],
            vec![tender("card", "80000"), tender("e_wallet", "30000")]),
        &tax, &StubBilling::default(), &StubPayment,
    ).await.unwrap_err();
    assert!(matches!(e, PosError::NonCashOverpayment { .. }), "got {e:?}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pos.pos_invoices WHERE client_uuid=$1")
        .bind(client).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0, "the refused replay wrote no ticket");

    // Card up to the total plus cash change is a valid replay.
    let out = w.sync_from_ui(
        sync(client, prof, session, vec![line(Uuid::new_v4(), "1", "100000")],
            vec![tender("card", "70000"), tender("cash", "50000")]),
        &tax, &StubBilling::default(), &StubPayment,
    ).await.unwrap();
    assert_eq!(out.totals.change_due, d("20000.00"));
}

/// PTR-4: only the replay that completes the payment triggers recognition. Replaying a ticket that
/// is already paid up does not re-emit or re-stage the trigger.
#[tokio::test]
async fn replaying_a_paid_up_draft_triggers_nothing() {
    let pool = pool().await;
    let rec = support::Recorder::default();
    let w = PosWriteService::with_sink(pool.clone(), Arc::new(rec.clone()));
    let (prof, tax) = register(&pool, false).await;
    let session = open(&w, prof).await;
    let client = Uuid::new_v4();
    let replay = || sync(client, prof, session, vec![line(Uuid::new_v4(), "1", "100000")], vec![tender("cash", "100000")]);

    let id = w.sync_from_ui(replay(), &tax, &StubBilling::default(), &StubPayment).await.unwrap().pos_invoice_id;
    assert_eq!(tender_completions(&rec.events, id), 1);
    w.sync_from_ui(replay(), &tax, &StubBilling::default(), &StubPayment).await.unwrap();
    w.sync_from_ui(replay(), &tax, &StubBilling::default(), &StubPayment).await.unwrap();
    assert_eq!(tender_completions(&rec.events, id), 1, "replays of a paid-up draft re-trigger nothing");
}

/// Billing that takes a while to raise, so two recognitions of one ticket overlap.
#[derive(Clone, Default)]
struct SlowBilling {
    raised: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl BillingPort for SlowBilling {
    async fn raise_and_post(&self, _req: &SaleInvoiceRequest) -> Result<InvoiceAck, PosRejected> {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        *self.raised.lock().unwrap() += 1;
        Ok(InvoiceAck { invoice_id: Uuid::new_v4(), journal_id: Uuid::new_v4(), grand_total: d("0") })
    }
    async fn credit_note(&self, _req: &CreditNoteRequest) -> Result<ReversalAck, PosRejected> {
        Ok(ReversalAck { journal_id: Uuid::new_v4() })
    }
}

/// PTR-5: two deliveries of the same recognition trigger raise one invoice. The second waits for the
/// first, then finds the ticket paid and answers with the same invoice.
#[tokio::test]
async fn parallel_recognitions_raise_one_invoice() {
    let pool = pool().await;
    let w = Arc::new(PosWriteService::new(pool.clone()));
    let (prof, tax) = register(&pool, false).await;
    let id = draft(&w, prof, &tax, vec![tender("cash", "100000")]).await;
    let billing = SlowBilling::default();

    let runs: Vec<_> = (0..2)
        .map(|_| {
            let (w, billing) = (w.clone(), billing.clone());
            tokio::spawn(async move { w.recognize_sale(id, &billing, &StubPayment, None).await })
        })
        .collect();
    let mut invoices = Vec::new();
    for r in runs {
        invoices.push(r.await.unwrap().unwrap().billing_invoice_id);
    }
    assert_eq!(*billing.raised.lock().unwrap(), 1, "one invoice raised for one sale");
    assert_eq!(invoices[0], invoices[1], "both deliveries answer with the same invoice");
}

/// Inventory that refuses its first `refusals` issues, then lands each one as a new note.
#[derive(Clone, Default)]
struct FlakyInventory {
    refusals: Arc<Mutex<usize>>,
    issued: Arc<Mutex<Vec<Uuid>>>,
}

#[async_trait::async_trait]
impl InventoryPort for FlakyInventory {
    async fn issue(&self, _req: &StockIssueRequest) -> Result<StockIssueAck, PosRejected> {
        {
            let mut left = self.refusals.lock().unwrap();
            if *left > 0 {
                *left -= 1;
                return Err(PosRejected { code: "insufficient_stock".into(), message: "not on hand".into() });
            }
        }
        let note = Uuid::new_v4();
        self.issued.lock().unwrap().push(note);
        Ok(StockIssueAck { delivery_note_id: note, journal_id: None })
    }
}

/// PTR-6: a stock relief refused after the ticket was marked paid is retried by the next delivery,
/// recorded on the ticket, and never issued twice.
#[tokio::test]
async fn a_refused_relief_is_retried_then_issued_once() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (prof, tax) = register(&pool, true).await;
    let id = draft(&w, prof, &tax, vec![tender("cash", "100000")]).await;
    let inventory = FlakyInventory { refusals: Arc::new(Mutex::new(1)), ..Default::default() };

    let e = w.recognize_sale(id, &StubBilling::default(), &StubPayment, Some(&inventory)).await.unwrap_err();
    assert!(matches!(e, PosError::InventoryRejected { .. }), "got {e:?}");
    let status: String = sqlx::query_scalar("SELECT status::text FROM pos.pos_invoices WHERE id=$1")
        .bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "paid", "the money is booked; only the relief is outstanding");

    w.recognize_sale(id, &StubBilling::default(), &StubPayment, Some(&inventory)).await.unwrap();
    w.recognize_sale(id, &StubBilling::default(), &StubPayment, Some(&inventory)).await.unwrap();
    let issued = inventory.issued.lock().unwrap().clone();
    assert_eq!(issued.len(), 1, "the retry issued the relief, and only once");
    let note: Option<Uuid> = sqlx::query_scalar("SELECT stock_delivery_note_id FROM pos.pos_invoices WHERE id=$1")
        .bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(note, Some(issued[0]), "the ticket records the note that relieved it");
}
