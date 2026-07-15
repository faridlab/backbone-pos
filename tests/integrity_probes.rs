//! Integrity probes for POS — invariants that must hold against a REAL Postgres, using trivial FAKE
//! ports (recognition math is tested in isolation; the real billing/payment seam is in
//! `retail_sale_seam.rs`). Requires DATABASE_URL (:5433/backbone_pos).

use std::sync::{Arc, Mutex};

use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_pos::application::service::pos_events::{PosEvent, PosEventSink};
use backbone_pos::application::service::pos_ports::{
    BillingPort, CreditNoteRequest, InvoiceAck, PaymentPort, PosRejected, RefundRequest, ReversalAck,
    SaleInvoiceRequest, SettlementAck, SettlementRequest,
};
use backbone_pos::application::service::pos_write_service::{NewSale, NewSaleLine, NewSession, PosError, PosWriteService};

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }
fn at() -> chrono::NaiveDateTime { chrono::NaiveDate::from_ymd_opt(2026, 7, 5).unwrap().and_hms_opt(9, 0, 0).unwrap() }
fn uq(p: &str) -> String { format!("{p}-{}", &Uuid::new_v4().simple().to_string()[..8]) }
async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5433/backbone_pos".to_string());
    PgPool::connect(&url).await.expect("connect DB")
}

/// A fake billing port that records how many times it was invoked + returns a fixed invoice.
#[derive(Default, Clone)]
struct FakeBilling { calls: Arc<Mutex<usize>>, invoice: Uuid }
#[async_trait::async_trait]
impl BillingPort for FakeBilling {
    async fn raise_and_post(&self, _req: &SaleInvoiceRequest) -> Result<InvoiceAck, PosRejected> {
        *self.calls.lock().unwrap() += 1;
        Ok(InvoiceAck { invoice_id: self.invoice, journal_id: Uuid::new_v4(), grand_total: d("0") })
    }
    async fn credit_note(&self, _req: &CreditNoteRequest) -> Result<ReversalAck, PosRejected> {
        Ok(ReversalAck { journal_id: Uuid::new_v4() })
    }
}
#[derive(Default, Clone)]
struct FakePayment;
#[async_trait::async_trait]
impl PaymentPort for FakePayment {
    async fn settle(&self, _req: &SettlementRequest) -> Result<SettlementAck, PosRejected> {
        Ok(SettlementAck { payment_id: Uuid::new_v4(), journal_id: Uuid::new_v4() })
    }
    async fn refund(&self, _req: &RefundRequest) -> Result<ReversalAck, PosRejected> {
        Ok(ReversalAck { journal_id: Uuid::new_v4() })
    }
}
/// A payment port that fails its FIRST `settle` (declined card) then succeeds — models the ordinary
/// decline-then-retry cashier workflow.
#[derive(Clone)]
struct FlakyPayment { failed: Arc<Mutex<bool>> }
#[async_trait::async_trait]
impl PaymentPort for FlakyPayment {
    async fn settle(&self, _req: &SettlementRequest) -> Result<SettlementAck, PosRejected> {
        let mut f = self.failed.lock().unwrap();
        if !*f { *f = true; return Err(PosRejected { code: "declined".into(), message: "card declined".into() }); }
        Ok(SettlementAck { payment_id: Uuid::new_v4(), journal_id: Uuid::new_v4() })
    }
    async fn refund(&self, _req: &RefundRequest) -> Result<ReversalAck, PosRejected> {
        Ok(ReversalAck { journal_id: Uuid::new_v4() })
    }
}
#[derive(Default, Clone)]
struct Recorder { events: Arc<Mutex<Vec<PosEvent>>> }
impl PosEventSink for Recorder { fn publish(&self, e: PosEvent) { self.events.lock().unwrap().push(e); } }

async fn profile(pool: &PgPool, company: Uuid, with_accounts: bool, customer: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    if with_accounts {
        sqlx::query(r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, receivable_account_id, income_account_id, cash_account_id, default_customer_id, allow_discount, is_active)
            VALUES ($1,$2,'R','IDR',$3,$4,$5,$6,true,true)"#)
            .bind(id).bind(company).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(customer).execute(pool).await.unwrap();
    } else {
        sqlx::query(r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, allow_discount, is_active) VALUES ($1,$2,'R','IDR',true,true)"#)
            .bind(id).bind(company).execute(pool).await.unwrap();
    }
    id
}
async fn ring(w: &PosWriteService, company: Uuid, prof: Uuid, session: Uuid, price: &str) -> Uuid {
    w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: uq("R"), posting_at: at(),
        lines: vec![NewSaleLine { item_id: Uuid::new_v4(), revenue_account_id: None, description: None, quantity: d("1"), unit_price: d(price), discount_amount: Decimal::ZERO }],
        tax_total: Decimal::ZERO, round_to: None,
    }).await.unwrap()
}
async fn session(w: &PosWriteService, company: Uuid, prof: Uuid) -> Uuid {
    w.open_session(NewSession { company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(), opened_at: at(), opening_balances: vec![] }).await.unwrap()
}

// IP-1: recognition is refused until the sale is fully tendered — no billing/payment is driven.
#[tokio::test]
async fn recognize_requires_full_tender() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let prof = profile(&pool, company, true, Some(Uuid::new_v4())).await;
    let s = session(&w, company, prof).await;
    let sale = ring(&w, company, prof, s, "100000").await;
    w.add_tender(sale, "cash", d("60000"), None).await.unwrap(); // partial
    let billing = FakeBilling { calls: Arc::new(Mutex::new(0)), invoice: Uuid::new_v4() };
    let e = w.recognize_sale(sale, &billing, &FakePayment, None).await.unwrap_err();
    assert!(matches!(e, PosError::NotFullyTendered { .. }));
    assert_eq!(*billing.calls.lock().unwrap(), 0, "billing is not driven for an unpaid ticket");
}

// IP-2: a profile without the register GL accounts cannot recognise a sale (fails before posting).
#[tokio::test]
async fn recognize_requires_register_accounts() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let prof = profile(&pool, company, false, None).await; // no accounts
    let s = session(&w, company, prof).await;
    let sale = ring(&w, company, prof, s, "100000").await;
    w.add_tender(sale, "cash", d("100000"), None).await.unwrap();
    let e = w.recognize_sale(sale, &FakeBilling::default(), &FakePayment, None).await.unwrap_err();
    assert!(matches!(e, PosError::MissingAccount(_)));
}

// IP-3: recognition is idempotent — a repeat call short-circuits on `paid`, driving billing/payment
// once and emitting `PosInvoicePaid` once (no double invoice / double settlement / double takings).
#[tokio::test]
async fn recognition_is_idempotent() {
    let pool = pool().await;
    let rec = Recorder::default();
    let w = PosWriteService::with_sink(pool.clone(), Arc::new(rec.clone()));
    let company = Uuid::new_v4();
    let customer = Uuid::new_v4();
    let prof = profile(&pool, company, true, Some(customer)).await;
    let s = session(&w, company, prof).await;
    let sale = ring(&w, company, prof, s, "100000").await;
    w.add_tender(sale, "cash", d("100000"), None).await.unwrap();
    let billing = FakeBilling { calls: Arc::new(Mutex::new(0)), invoice: Uuid::new_v4() };

    let first = w.recognize_sale(sale, &billing, &FakePayment, None).await.unwrap();
    let second = w.recognize_sale(sale, &billing, &FakePayment, None).await.unwrap();
    assert_eq!(first.billing_invoice_id, second.billing_invoice_id);
    assert_eq!(*billing.calls.lock().unwrap(), 1, "billing driven exactly once across two recognises");
    let paid = rec.events.lock().unwrap().iter().filter(|e| matches!(e, PosEvent::PosInvoicePaid(p) if p.pos_invoice_id == sale)).count();
    assert_eq!(paid, 1, "PosInvoicePaid emitted exactly once");
}

// IP-4 (council 2026-07-05): billing is raised AT MOST ONCE even when the tender fails then retries.
// A declined card leaves the ticket `draft` with the Sales Invoice already raised + linked; the retry
// REUSES it (skips billing) rather than raising a second invoice — closing the double-revenue the
// ordinary decline-then-retry workflow would otherwise book.
#[tokio::test]
async fn billing_raised_at_most_once_across_decline_then_retry() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let customer = Uuid::new_v4();
    let prof = profile(&pool, company, true, Some(customer)).await;
    let s = session(&w, company, prof).await;
    let sale = ring(&w, company, prof, s, "100000").await;
    w.add_tender(sale, "cash", d("100000"), None).await.unwrap();
    let billing = FakeBilling { calls: Arc::new(Mutex::new(0)), invoice: Uuid::new_v4() };
    let payment = FlakyPayment { failed: Arc::new(Mutex::new(false)) };

    // 1) tender fails → recognise errors, but billing WAS raised + linked (mid-flight persist).
    let e = w.recognize_sale(sale, &billing, &payment, None).await.unwrap_err();
    assert!(matches!(e, PosError::PaymentRejected { .. }));
    let (st, bid): (String, Option<Uuid>) = sqlx::query_as("SELECT status::text, billing_invoice_id FROM pos.pos_invoices WHERE id=$1").bind(sale).fetch_one(&pool).await.unwrap();
    assert_eq!(st, "draft", "still unpaid after the decline");
    assert_eq!(bid, Some(billing.invoice), "the raised invoice is persisted while draft");
    assert_eq!(*billing.calls.lock().unwrap(), 1);

    // 2) retry with the now-succeeding tender → REUSES the invoice (no second billing raise).
    w.recognize_sale(sale, &billing, &payment, None).await.unwrap();
    assert_eq!(*billing.calls.lock().unwrap(), 1, "billing raised exactly once — no double revenue on retry");
    let st2: String = sqlx::query_scalar("SELECT status::text FROM pos.pos_invoices WHERE id=$1").bind(sale).fetch_one(&pool).await.unwrap();
    assert_eq!(st2, "paid");
}
