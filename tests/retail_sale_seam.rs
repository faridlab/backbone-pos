//! The retail SALE seam, end-to-end across FOUR modules: **POS → billing → payment → accounting** —
//! the counter transaction that recognises revenue AND settles tender in one flow. Zero normal Cargo
//! edges (billing/payment/accounting are dev-deps only; POS drives them through `BillingPort` /
//! `PaymentPort`).
//!
//! Flow: POS rings a sale + takes cash tender → `recognize_sale` hands billing a `SaleInvoiceRequest`
//! (billing raises the real Sales Invoice + posts revenue `Dr A/R · Cr Revenue`) then payment a
//! `SettlementRequest` (payment posts `Dr Cash · Cr A/R` + billing draws the invoice down). The
//! **A/R control nets to zero** at the counter, cash holds the takings, the ticket is `paid`. All four
//! schemas co-locate in one DB. Requires DATABASE_URL (:5433/backbone_pos).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_pos::application::service::pos_events::{PosEvent, PosEventSink};
use backbone_pos::application::service::pos_ports::{
    BillingPort, CreditNoteRequest, InventoryPort, InvoiceAck, PaymentPort, PosRejected, RefundRequest,
    ReversalAck, SaleInvoiceRequest, SettlementAck, SettlementRequest, StockIssueAck, StockIssueRequest,
};
use backbone_pos::application::service::pos_write_service::{NewClose, NewSale, NewSaleLine, NewSession, PosWriteService};

use backbone_billing::application::service::billing_gl::{
    AccountingPostEnvelope as BillEnv, GlPostAck as BillAck, GlPostRejected as BillRej, GlPostSink as BillSink,
};
use backbone_billing::application::service::billing_write_service::{
    BillingWriteService, NewInvoiceLine, NewSalesInvoice, NewTaxLine,
};
use backbone_payment::application::service::payment_gl::{
    AccountingPostEnvelope as PayEnv, GlPostAck as PayAck, GlPostRejected as PayRej, GlPostSink as PaySink,
};
use backbone_payment::application::service::payment_write_service::{NewAllocation, NewPayment, PaymentWriteService};

use backbone_accounting::application::service::posting_service::{PostingLine, PostingRequest, PostingService};

use backbone_inventory::application::service::inventory_gl::{
    AccountingPostEnvelope as InvEnv, GlPostAck as InvAck, GlPostRejected as InvRej, GlPostSink as InvSink,
};
use backbone_inventory::application::service::inventory_write_service::{
    DeliveryLine, InventoryWriteService, NewDelivery, NewReceipt, NewStockItem, NewWarehouse,
    ReceiptLine as InvReceiptLine,
};

/// ACL: either producer's serialized envelope → accounting's PostingRequest against the REAL ledger.
struct GlAdapter { svc: PostingService }
impl GlAdapter {
    #[allow(clippy::too_many_arguments)]
    async fn go(&self, company: Uuid, source_type: &str, source_id: Uuid, reference: Option<String>, date: chrono::NaiveDate, posting_type: &str, reverses_post_id: Option<Uuid>, lines: Vec<PostingLine>) -> Result<(Uuid, Uuid, bool), (String, String)> {
        let mut r = PostingRequest::original(company, source_type, source_id, date);
        r.source_reference = reference; r.posting_type = posting_type.to_string(); r.reverses_post_id = reverses_post_id; r.lines = lines;
        match self.svc.post(r, None).await {
            Ok(x) => Ok((x.post_id, x.journal_id, x.idempotent_reuse)),
            Err(x) => Err((x.code().to_string(), x.to_string())),
        }
    }
}
#[async_trait::async_trait]
impl BillSink for GlAdapter {
    async fn post(&self, e: &BillEnv) -> Result<BillAck, BillRej> {
        let lines = e.lines.iter().map(pl).collect();
        match self.go(e.company_id, &e.source_type, e.source_id, e.source_reference.clone(), e.posting_date, &e.posting_type, e.reverses_post_id, lines).await {
            Ok((post_id, journal_id, idempotent_reuse)) => Ok(BillAck { post_id, journal_id, idempotent_reuse }),
            Err((code, message)) => Err(BillRej { code, message }),
        }
    }
}
#[async_trait::async_trait]
impl PaySink for GlAdapter {
    async fn post(&self, e: &PayEnv) -> Result<PayAck, PayRej> {
        let lines = e.lines.iter().map(pl_pay).collect();
        match self.go(e.company_id, &e.source_type, e.source_id, e.source_reference.clone(), e.posting_date, &e.posting_type, e.reverses_post_id, lines).await {
            Ok((post_id, journal_id, idempotent_reuse)) => Ok(PayAck { post_id, journal_id, idempotent_reuse }),
            Err((code, message)) => Err(PayRej { code, message }),
        }
    }
}
fn pl(l: &backbone_billing::application::service::billing_gl::GlPostLine) -> PostingLine {
    PostingLine { account_id: l.account_id, debit: l.debit, credit: l.credit, party_type: l.party_type.clone(), party_id: l.party_id, cost_center_id: None, project_id: None, department_id: None, description: l.description.clone() }
}
fn pl_pay(l: &backbone_payment::application::service::payment_gl::GlPostLine) -> PostingLine {
    PostingLine { account_id: l.account_id, debit: l.debit, credit: l.credit, party_type: l.party_type.clone(), party_id: l.party_id, cost_center_id: None, project_id: None, department_id: None, description: l.description.clone() }
}
#[async_trait::async_trait]
impl InvSink for GlAdapter {
    async fn post(&self, e: &InvEnv) -> Result<InvAck, InvRej> {
        let lines = e.lines.iter().map(|l| PostingLine {
            account_id: l.account_id, debit: l.debit, credit: l.credit, party_type: l.party_type.clone(),
            party_id: l.party_id, cost_center_id: None, project_id: None, department_id: None, description: l.description.clone(),
        }).collect();
        match self.go(e.company_id, &e.source_type, e.source_id, e.source_reference.clone(), e.posting_date, &e.posting_type, None, lines).await {
            Ok((post_id, journal_id, idempotent_reuse)) => Ok(InvAck { post_id, journal_id, idempotent_reuse }),
            Err((code, message)) => Err(InvRej { code, message }),
        }
    }
}

/// InventoryPort over the real inventory service: issue an outward Delivery Note (relieve on-hand +
/// post `Dr COGS · Cr Inventory` at moving-average cost).
struct InventoryAdapter { inv: InventoryWriteService, gl: Arc<GlAdapter> }
#[async_trait::async_trait]
impl InventoryPort for InventoryAdapter {
    async fn issue(&self, req: &StockIssueRequest) -> Result<StockIssueAck, PosRejected> {
        let id = self.inv.create_delivery_note(NewDelivery {
            delivery_number: uq("DN"), company_id: req.company_id, branch_id: req.branch_id, customer_id: req.customer_id,
            source_so_id: Some(req.source_pos_id), warehouse_id: req.warehouse_id, posting_date: req.posting_date,
            cogs_account_id: req.cogs_account_id, inventory_account_id: req.inventory_account_id,
            lines: req.lines.iter().map(|l| DeliveryLine { item_id: l.item_id, quantity: l.quantity }).collect(),
        }).await.map_err(|e| PosRejected { code: e.code(), message: e.to_string() })?;
        let out = self.inv.submit_delivery_note(id, &*self.gl).await.map_err(|e| PosRejected { code: e.code(), message: e.to_string() })?;
        Ok(StockIssueAck { delivery_note_id: id, journal_id: out.journal_id })
    }
}

/// BillingPort over the real billing service: raise + post the Sales Invoice.
struct BillingAdapter { billing: BillingWriteService, gl: Arc<GlAdapter> }
#[async_trait::async_trait]
impl BillingPort for BillingAdapter {
    async fn raise_and_post(&self, req: &SaleInvoiceRequest) -> Result<InvoiceAck, PosRejected> {
        let net: Decimal = req.lines.iter().map(|l| l.quantity * l.unit_price).sum();
        let grand: Decimal = net + req.tax_total;
        // PPN: when POS supplies a tax total + its output-tax account, book it as an `output` tax line so
        // billing posts `Dr A/R (grand) · Cr Revenue (net) · Cr PPN Output (tax)`.
        let tax_lines = match (req.tax_account_id, req.tax_total > Decimal::ZERO) {
            (Some(account_id), true) => vec![NewTaxLine {
                account_id, basis: "output".into(), description: Some("PPN".into()),
                rate: req.tax_rate, tax_amount: req.tax_total,
            }],
            _ => vec![],
        };
        let inv = self.billing.create_sales_invoice(NewSalesInvoice {
            invoice_number: uq("SI"), company_id: req.company_id, branch_id: None, customer_id: req.customer_id,
            source_so_id: Some(req.source_pos_id), posting_date: req.posting_date, due_date: None,
            currency: Some(req.currency.clone()), receivable_account_id: req.receivable_account_id,
            lines: req.lines.iter().map(|l| NewInvoiceLine { item_id: l.item_id, account_id: l.revenue_account_id, description: None, quantity: l.quantity, unit_price: l.unit_price }).collect(),
            tax_lines,
        }).await.map_err(|e| PosRejected { code: e.code(), message: e.to_string() })?;
        let out = self.billing.post_sales_invoice(inv, &*self.gl).await.map_err(|e| PosRejected { code: e.code(), message: e.to_string() })?;
        Ok(InvoiceAck { invoice_id: inv, journal_id: out.journal_id, grand_total: grand })
    }
    async fn credit_note(&self, req: &CreditNoteRequest) -> Result<ReversalAck, PosRejected> {
        // Reverse the revenue: Dr Revenue · Cr A/R, invoice → cancelled.
        let out = self.billing.reverse_sales_invoice(req.invoice_ref, &*self.gl).await.map_err(|e| PosRejected { code: e.code(), message: e.to_string() })?;
        Ok(ReversalAck { journal_id: out.journal_id })
    }
}
/// PaymentPort over the real payment service: settle the tender + draw the invoice down in billing.
///
/// Probe (2026-07-14): this adapter no longer holds a `pool`. The refund path used to resolve the
/// settling payment with a cross-schema `SELECT ... FROM payment.payment_allocations` — reading another
/// module's private table, which only works when all four schemas co-locate in one DB. `RefundRequest`
/// now carries `payment_id` (persisted by POS on the ticket at recognition), so the refund is
/// self-contained and would survive payment running on its own database/bus.
struct PaymentAdapter { payment: PaymentWriteService, billing: BillingWriteService, gl: Arc<GlAdapter> }
#[async_trait::async_trait]
impl PaymentPort for PaymentAdapter {
    async fn settle(&self, req: &SettlementRequest) -> Result<SettlementAck, PosRejected> {
        let pay = self.payment.create_payment(NewPayment {
            payment_number: uq("PE"), company_id: req.company_id, branch_id: None, payment_type: "receive".into(),
            party_type: Some("customer".into()), party_id: Some(req.customer_id), posting_date: req.posting_date,
            currency: Some(req.currency.clone()), mode_of_payment_id: None, bank_account_id: req.bank_account_id,
            party_account_id: req.party_account_id, paid_amount: req.amount, reference_no: None,
            allocations: vec![NewAllocation { invoice_ref: req.invoice_ref, invoice_kind: "sales".into(), amount: req.amount }],
        }).await.map_err(|e| PosRejected { code: e.code(), message: e.to_string() })?;
        let out = self.payment.post_payment(pay, &*self.gl).await.map_err(|e| PosRejected { code: e.code(), message: e.to_string() })?;
        self.billing.apply_settlement(req.invoice_ref, "sales", req.amount).await.map_err(|e| PosRejected { code: e.code(), message: e.to_string() })?;
        Ok(SettlementAck { payment_id: pay, journal_id: out.journal_id })
    }
    async fn refund(&self, req: &RefundRequest) -> Result<ReversalAck, PosRejected> {
        // Reverse the settling payment (Dr A/R · Cr Cash), restore outstanding. The payment_id comes
        // from the request (POS persisted it on the ticket at recognition) — no cross-schema read into
        // payment.payment_allocations, so this refund is satisfiable with payment on its own database.
        let out = self.payment.reverse_payment(req.payment_id, &*self.gl).await.map_err(|e| PosRejected { code: e.code(), message: e.to_string() })?;
        self.billing.reverse_settlement(req.invoice_ref, "sales", req.amount).await.map_err(|e| PosRejected { code: e.code(), message: e.to_string() })?;
        Ok(ReversalAck { journal_id: out.journal_id })
    }
}

#[derive(Default, Clone)]
struct RecordingPosSink { events: Arc<Mutex<Vec<PosEvent>>> }
impl PosEventSink for RecordingPosSink { fn publish(&self, e: PosEvent) { self.events.lock().unwrap().push(e); } }

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }
fn at() -> chrono::NaiveDateTime { chrono::NaiveDate::from_ymd_opt(2026, 7, 5).unwrap().and_hms_opt(9, 0, 0).unwrap() }
fn uq(p: &str) -> String { format!("{p}-{}", &Uuid::new_v4().simple().to_string()[..8]) }
async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5433/backbone_pos".to_string());
    PgPool::connect(&url).await.expect("connect DB")
}
async fn seed_coa(pool: &PgPool) -> (Uuid, HashMap<&'static str, Uuid>) {
    let company = Uuid::new_v4();
    let coa: &[(&str, &str, &str, &str, &str)] = &[
        ("1200", "Piutang Usaha", "asset", "accounts_receivable", "debit"),
        ("4000", "Pendapatan Ritel", "revenue", "operating_revenue", "credit"),
        ("1110", "Kas", "asset", "cash", "debit"),
    ];
    let mut m = HashMap::new();
    for (code, name, a, s, nb) in coa {
        let id = Uuid::new_v4();
        sqlx::query(r#"INSERT INTO accounting.accounts (id, company_id, account_number, account_code, name, account_type, account_subtype, normal_balance, is_header, is_detail, status)
            VALUES ($1,$2,$3,$4,$5,$6::account_type,$7::account_subtype,$8::normal_balance,false,true,'active'::account_status)"#)
            .bind(id).bind(company).bind(code).bind(code).bind(name).bind(a).bind(s).bind(nb).execute(pool).await.expect("seed acct");
        m.insert(*code, id);
    }
    (company, m)
}
async fn balance(pool: &PgPool, account: Uuid) -> Decimal {
    sqlx::query_scalar("SELECT COALESCE(SUM(debit_amount),0) - COALESCE(SUM(credit_amount),0) FROM accounting.ledgers WHERE account_id=$1")
        .bind(account).fetch_one(pool).await.unwrap()
}
async fn on_hand(pool: &PgPool, item: Uuid, warehouse: Uuid) -> Decimal {
    sqlx::query_scalar("SELECT COALESCE(SUM(actual_qty),0) FROM inventory.bins WHERE item_id=$1 AND warehouse_id=$2")
        .bind(item).bind(warehouse).fetch_one(pool).await.unwrap()
}

/// RSSEAM-1: a cash retail sale recognised across POS, billing, payment, and the real ledger.
#[tokio::test]
async fn retail_cash_sale_across_four_modules() {
    let pool = pool().await;
    let (company, coa) = seed_coa(&pool).await;
    let customer = Uuid::new_v4();
    let item = Uuid::new_v4();

    // POS profile wired to the register's GL accounts + a default walk-in customer.
    let prof = Uuid::new_v4();
    sqlx::query(r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, receivable_account_id, income_account_id, cash_account_id, default_customer_id, allow_discount, is_active)
        VALUES ($1,$2,'Register 1','IDR',$3,$4,$5,$6,true,true)"#)
        .bind(prof).bind(company).bind(coa["1200"]).bind(coa["4000"]).bind(coa["1110"]).bind(customer).execute(&pool).await.unwrap();

    let rec = RecordingPosSink::default();
    let pos = PosWriteService::with_sink(pool.clone(), Arc::new(rec.clone()));
    let gl = Arc::new(GlAdapter { svc: PostingService::new(pool.clone()) });
    let billing_port = BillingAdapter { billing: BillingWriteService::new(pool.clone()), gl: gl.clone() };
    let payment_port = PaymentAdapter { payment: PaymentWriteService::new(pool.clone()), billing: BillingWriteService::new(pool.clone()), gl: gl.clone() };

    // 1) open the till, ring a 100,000 sale, take 100,000 cash.
    let session = pos.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![("cash".into(), d("500000"))],
    }).await.unwrap();
    let sale = pos.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: uq("R"), posting_at: at(),
        lines: vec![NewSaleLine { item_id: item, revenue_account_id: None, description: None, quantity: d("1"), unit_price: d("100000"), discount_amount: Decimal::ZERO }],
        tax_total: Decimal::ZERO, round_to: None,
    }).await.unwrap();
    let t = pos.add_tender(sale, "cash", d("100000"), None).await.unwrap();
    assert!(t.fully_tendered);

    // 2) recognise — POS drives billing (revenue) + payment (settlement) into the real ledger.
    let out = pos.recognize_sale(sale, &billing_port, &payment_port, None).await.unwrap();

    // 3) the counter transaction booked BOTH legs and the A/R nets to zero.
    assert_eq!(balance(&pool, coa["1200"]).await, d("0.00"), "A/R nets to zero at the counter");
    assert_eq!(balance(&pool, coa["4000"]).await, d("-100000.00"), "revenue recognised (credit)");
    assert_eq!(balance(&pool, coa["1110"]).await, d("100000.00"), "cash holds the takings");
    // ticket paid + linked; billing invoice paid.
    let (st, bid): (String, Option<Uuid>) = sqlx::query_as("SELECT status::text, billing_invoice_id FROM pos.pos_invoices WHERE id=$1").bind(sale).fetch_one(&pool).await.unwrap();
    assert_eq!(st, "paid");
    assert_eq!(bid, Some(out.billing_invoice_id));
    let inv_st: String = sqlx::query_scalar("SELECT status::text FROM billing.sales_invoices WHERE id=$1").bind(out.billing_invoice_id).fetch_one(&pool).await.unwrap();
    assert_eq!(inv_st, "paid");
    assert!(rec.events.lock().unwrap().iter().any(|e| matches!(e, PosEvent::PosInvoicePaid(p) if p.pos_invoice_id == sale)));

    // 4) close the till — expected cash = opening 500,000 + tender 100,000 = 600,000; counted balances.
    let close = pos.close_session(NewClose {
        company_id: company, opening_entry_id: session, cashier_party_id: Uuid::new_v4(), closed_at: at(),
        counted: vec![("cash".into(), d("600000"))],
    }).await.unwrap();
    assert_eq!(close.difference_total, d("0.00"));
    let cash = close.by_method.iter().find(|r| r.method == "cash").unwrap();
    assert_eq!(cash.expected, d("600000.00"), "opening float + recognised cash tender");
}

/// RSSEAM-2 (completeness council 2026-07-05): a return reverses BOTH legs — POS drives billing
/// (credit note `Dr Revenue · Cr A/R`) + payment (refund `Dr A/R · Cr Cash`) — so revenue, cash, AND
/// A/R all net back to zero. Idempotent: a repeat `return_sale` refunds/credits at most once.
#[tokio::test]
async fn retail_return_reverses_both_legs_and_is_idempotent() {
    let pool = pool().await;
    let (company, coa) = seed_coa(&pool).await;
    let customer = Uuid::new_v4();
    let item = Uuid::new_v4();

    let pos = PosWriteService::new(pool.clone());
    let gl = Arc::new(GlAdapter { svc: PostingService::new(pool.clone()) });
    let billing_port = BillingAdapter { billing: BillingWriteService::new(pool.clone()), gl: gl.clone() };
    let payment_port = PaymentAdapter { payment: PaymentWriteService::new(pool.clone()), billing: BillingWriteService::new(pool.clone()), gl: gl.clone() };
    let prof = Uuid::new_v4();
    sqlx::query(r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, receivable_account_id, income_account_id, cash_account_id, default_customer_id, allow_discount, is_active)
        VALUES ($1,$2,'R','IDR',$3,$4,$5,$6,true,true)"#)
        .bind(prof).bind(company).bind(coa["1200"]).bind(coa["4000"]).bind(coa["1110"]).bind(customer).execute(&pool).await.unwrap();

    // A recognised 100,000 cash sale (A/R 0, Revenue -100k, Cash +100k).
    let session = pos.open_session(NewSession { company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(), opened_at: at(), opening_balances: vec![] }).await.unwrap();
    let sale = pos.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: uq("R"), posting_at: at(),
        lines: vec![NewSaleLine { item_id: item, revenue_account_id: None, description: None, quantity: d("1"), unit_price: d("100000"), discount_amount: Decimal::ZERO }],
        tax_total: Decimal::ZERO, round_to: None,
    }).await.unwrap();
    pos.add_tender(sale, "cash", d("100000"), None).await.unwrap();
    pos.recognize_sale(sale, &billing_port, &payment_port, None).await.unwrap();
    assert_eq!(balance(&pool, coa["4000"]).await, d("-100000.00"));

    // Return it — both legs reverse; revenue, cash, A/R all net to ZERO.
    let ret = pos.return_sale(sale, &billing_port, &payment_port).await.unwrap();
    assert_eq!(balance(&pool, coa["1200"]).await, d("0.00"), "A/R still zero after return");
    assert_eq!(balance(&pool, coa["4000"]).await, d("0.00"), "revenue reversed to zero");
    assert_eq!(balance(&pool, coa["1110"]).await, d("0.00"), "cash refunded to zero");
    // original ticket returned; billing invoice cancelled; a return ticket recorded.
    assert_eq!(sqlx::query_scalar::<_, String>("SELECT status::text FROM pos.pos_invoices WHERE id=$1").bind(sale).fetch_one(&pool).await.unwrap(), "returned");
    assert_eq!(sqlx::query_scalar::<_, String>("SELECT status::text FROM billing.sales_invoices WHERE id=$1").bind(ret.billing_invoice_id).fetch_one(&pool).await.unwrap(), "cancelled");
    let (is_ret, against): (bool, Option<Uuid>) = sqlx::query_as("SELECT is_return, return_against FROM pos.pos_invoices WHERE id=$1").bind(ret.return_ticket_id).fetch_one(&pool).await.unwrap();
    assert!(is_ret);
    assert_eq!(against, Some(sale));

    // Idempotent: a second return does not double-refund — GL stays at zero, one return ticket only.
    pos.return_sale(sale, &billing_port, &payment_port).await.unwrap();
    assert_eq!(balance(&pool, coa["1110"]).await, d("0.00"), "no double refund");
    assert_eq!(balance(&pool, coa["4000"]).await, d("0.00"), "no double credit note");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pos.pos_invoices WHERE return_against=$1 AND is_return=true").bind(sale).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1, "exactly one return ticket");
}

/// RSSEAM-3 (PPN, 2026-07-14): a PKP register charges 11% output tax, computed SERVER-side from the
/// profile (the client sends no tax). Billing books `Dr A/R 111,000 · Cr Revenue 100,000 · Cr PPN
/// Output 11,000`; payment settles the grand in cash. A/R nets to zero, revenue holds the net, cash
/// holds the grand, and the PPN liability holds the tax.
#[tokio::test]
async fn retail_ppn_sale_books_output_tax() {
    let pool = pool().await;
    let (company, coa) = seed_coa(&pool).await;
    let customer = Uuid::new_v4();
    let item = Uuid::new_v4();

    // PPN output-tax liability account (credit-normal).
    let ppn = Uuid::new_v4();
    sqlx::query(r#"INSERT INTO accounting.accounts (id, company_id, account_number, account_code, name, account_type, account_subtype, normal_balance, is_header, is_detail, status)
        VALUES ($1,$2,'2130','2130','Utang PPN Keluaran','liability'::account_type,'tax'::account_subtype,'credit'::normal_balance,false,true,'active'::account_status)"#)
        .bind(ppn).bind(company).execute(&pool).await.unwrap();

    // PKP register: 11% PPN wired to the output-tax account.
    let prof = Uuid::new_v4();
    sqlx::query(r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, receivable_account_id, income_account_id, cash_account_id, tax_account_id, tax_rate, default_customer_id, allow_discount, is_active)
        VALUES ($1,$2,'PKP Register','IDR',$3,$4,$5,$6,0.1100,$7,true,true)"#)
        .bind(prof).bind(company).bind(coa["1200"]).bind(coa["4000"]).bind(coa["1110"]).bind(ppn).bind(customer).execute(&pool).await.unwrap();

    let pos = PosWriteService::new(pool.clone());
    let gl = Arc::new(GlAdapter { svc: PostingService::new(pool.clone()) });
    let billing_port = BillingAdapter { billing: BillingWriteService::new(pool.clone()), gl: gl.clone() };
    let payment_port = PaymentAdapter { payment: PaymentWriteService::new(pool.clone()), billing: BillingWriteService::new(pool.clone()), gl: gl.clone() };

    let session = pos.open_session(NewSession { company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(), opened_at: at(), opening_balances: vec![] }).await.unwrap();
    let sale = pos.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: uq("R"), posting_at: at(),
        lines: vec![NewSaleLine { item_id: item, revenue_account_id: None, description: None, quantity: d("1"), unit_price: d("100000"), discount_amount: Decimal::ZERO }],
        tax_total: Decimal::ZERO, round_to: None, // ignored — server computes PPN from the profile
    }).await.unwrap();

    // The ticket carries server-computed PPN: net 100,000, tax 11,000, grand 111,000.
    let (net, tax, grand): (Decimal, Decimal, Decimal) = sqlx::query_as("SELECT net_total, tax_total, rounded_total FROM pos.pos_invoices WHERE id=$1").bind(sale).fetch_one(&pool).await.unwrap();
    assert_eq!((net, tax, grand), (d("100000.00"), d("11000.00"), d("111000.00")), "server-side PPN on the ticket");

    let t = pos.add_tender(sale, "cash", d("111000"), None).await.unwrap();
    assert!(t.fully_tendered);
    pos.recognize_sale(sale, &billing_port, &payment_port, None).await.unwrap();

    assert_eq!(balance(&pool, coa["1200"]).await, d("0.00"), "A/R nets to zero at the counter");
    assert_eq!(balance(&pool, coa["4000"]).await, d("-100000.00"), "revenue holds the net (credit)");
    assert_eq!(balance(&pool, coa["1110"]).await, d("111000.00"), "cash holds the grand (incl PPN)");
    assert_eq!(balance(&pool, ppn).await, d("-11000.00"), "PPN output-tax liability holds the tax (credit)");
}

/// RSSEAM-4 (stock, 2026-07-15): a stock-tracking register relieves on-hand and books COGS on the sale.
/// Seed 10 units @ 60,000 via a purchase receipt; ring + recognise 2 units through the InventoryPort →
/// on-hand falls to 8 and `Dr COGS 120,000 · Cr Inventory 120,000` posts at moving-average cost.
#[tokio::test]
async fn retail_sale_decrements_stock_and_books_cogs() {
    let pool = pool().await;
    let (company, coa) = seed_coa(&pool).await;
    let customer = Uuid::new_v4();
    let item = Uuid::new_v4();

    // Inventory (asset), COGS (cogs), GRIR (liability) accounts.
    let inv_acct = Uuid::new_v4();
    let cogs_acct = Uuid::new_v4();
    let grir_acct = Uuid::new_v4();
    for (id, code, name, a, s, nb) in [
        (inv_acct, "1300", "Persediaan", "asset", "inventory", "debit"),
        (cogs_acct, "5000", "HPP", "cogs", "direct_cost", "debit"),
        (grir_acct, "2140", "GRIR", "liability", "current_liability", "credit"),
    ] {
        sqlx::query(r#"INSERT INTO accounting.accounts (id, company_id, account_number, account_code, name, account_type, account_subtype, normal_balance, is_header, is_detail, status)
            VALUES ($1,$2,$3,$4,$5,$6::account_type,$7::account_subtype,$8::normal_balance,false,true,'active'::account_status)"#)
            .bind(id).bind(company).bind(code).bind(code).bind(name).bind(a).bind(s).bind(nb).execute(&pool).await.unwrap();
    }

    let inv = InventoryWriteService::new(pool.clone());
    let gl = Arc::new(GlAdapter { svc: PostingService::new(pool.clone()) });

    // Warehouse + stock item + opening on-hand: receive 10 @ 60,000.
    let wh = inv.create_warehouse(NewWarehouse { company_id: company, code: uq("WH"), name: "Toko".into(), warehouse_type: None, parent_warehouse_id: None, is_group: false }).await.unwrap();
    inv.create_stock_item(NewStockItem { item_id: item, company_id: company, stock_uom: "PCS".into(), valuation_method: None, reorder_level: Decimal::ZERO }).await.unwrap();
    let receipt = inv.create_purchase_receipt(NewReceipt {
        receipt_number: uq("PR"), company_id: company, branch_id: None, supplier_id: Uuid::new_v4(), source_po_id: None,
        warehouse_id: wh, posting_date: at().date(), inventory_account_id: inv_acct, grir_account_id: grir_acct,
        lines: vec![InvReceiptLine { item_id: item, quantity: d("10"), rate: d("60000") }],
    }).await.unwrap();
    inv.submit_purchase_receipt(receipt, &*gl).await.unwrap();
    assert_eq!(on_hand(&pool, item, wh).await, d("10.0000"), "seeded on-hand");

    // POS profile: stock-tracking register.
    let prof = Uuid::new_v4();
    sqlx::query(r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, receivable_account_id, income_account_id, cash_account_id, warehouse_id, cogs_account_id, inventory_account_id, default_customer_id, allow_discount, is_active)
        VALUES ($1,$2,'Register 1','IDR',$3,$4,$5,$6,$7,$8,$9,true,true)"#)
        .bind(prof).bind(company).bind(coa["1200"]).bind(coa["4000"]).bind(coa["1110"]).bind(wh).bind(cogs_acct).bind(inv_acct).bind(customer).execute(&pool).await.unwrap();

    let pos = PosWriteService::new(pool.clone());
    let billing_port = BillingAdapter { billing: BillingWriteService::new(pool.clone()), gl: gl.clone() };
    let payment_port = PaymentAdapter { payment: PaymentWriteService::new(pool.clone()), billing: BillingWriteService::new(pool.clone()), gl: gl.clone() };
    let inventory_port = InventoryAdapter { inv: InventoryWriteService::new(pool.clone()), gl: gl.clone() };

    let session = pos.open_session(NewSession { company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(), opened_at: at(), opening_balances: vec![] }).await.unwrap();
    let sale = pos.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        receipt_number: uq("R"), posting_at: at(),
        lines: vec![NewSaleLine { item_id: item, revenue_account_id: None, description: None, quantity: d("2"), unit_price: d("100000"), discount_amount: Decimal::ZERO }],
        tax_total: Decimal::ZERO, round_to: None,
    }).await.unwrap();
    pos.add_tender(sale, "cash", d("200000"), None).await.unwrap();
    pos.recognize_sale(sale, &billing_port, &payment_port, Some(&inventory_port)).await.unwrap();

    // On-hand fell by the 2 sold; COGS booked at moving-average 60,000 → 120,000; inventory nets 480k.
    assert_eq!(on_hand(&pool, item, wh).await, d("8.0000"), "on-hand decremented by the sale");
    assert_eq!(balance(&pool, cogs_acct).await, d("120000.00"), "COGS debited at cost");
    assert_eq!(balance(&pool, inv_acct).await, d("480000.00"), "inventory = 10*60k received − 2*60k issued");
}
