//! Document-grade tax through `PosTaxComputePort`: the register's templates name WHAT applies, the
//! port resolves HOW it taxes, and the compute core owns every money field written. POS-only.
//! Requires DATABASE_URL (:5433/backbone_pos).

mod support;
use support::{at, d, pool, seed_profile, uq, TestTax};

use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_pos::application::service::pos_write_service::{
    NewSale, NewSaleLine, NewSession, PosError, PosWriteService,
};

async fn open(w: &PosWriteService, company: Uuid, prof: Uuid) -> Uuid {
    w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at: at(), opening_balances: vec![],
    }).await.unwrap()
}
fn line(item: Uuid, qty: &str, price: &str) -> NewSaleLine {
    NewSaleLine { item_id: item, revenue_account_id: None, description: None, quantity: d(qty), unit_price: d(price), course: None, discount_amount: Decimal::ZERO }
}
async fn ring(w: &PosWriteService, company: Uuid, prof: Uuid, session: Uuid, tax: &TestTax, lines: Vec<NewSaleLine>) -> Result<Uuid, PosError> {
    w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None,
        receipt_number: uq("R"), posting_at: at(), lines,
    }, tax).await
}

// TC-1: an 11% template taxes the NET after discount — 2 × 50,000 − 5,000 = 95,000 net, tax 10,450,
// grand 105,450 — all server-derived, nothing read from the client.
#[tokio::test]
async fn document_grade_tax_on_the_discounted_net() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::profile_at_rate(&pool, company, "0.11").await;
    let session = open(&w, company, prof).await;
    let sale = ring(&w, company, prof, session, &tax, vec![
        NewSaleLine { discount_amount: d("5000"), ..line(item, "2", "50000") },
    ]).await.unwrap();
    let (net, tax_total, grand, rounded): (Decimal, Decimal, Decimal, Decimal) =
        sqlx::query_as("SELECT net_total, tax_total, grand_total, rounded_total FROM pos.pos_invoices WHERE id=$1")
            .bind(sale).fetch_one(&pool).await.unwrap();
    assert_eq!((net, tax_total, grand, rounded), (d("95000.00"), d("10450.00"), d("105450.00"), d("105450.00")));
}

// TC-2: multiple templates on one register all apply to every line (11% + 1% → 12% total), and the
// header tax total equals the SUM OF COMPONENTS the implementer would book.
#[tokio::test]
async fn multiple_templates_sum_into_the_tax_total() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (t11, t1) = (Uuid::new_v4(), Uuid::new_v4());
    let prof = seed_profile(&pool, company, &[t11, t1], None).await;
    let tax = TestTax::with_rates(vec![(t11, "0.11"), (t1, "0.01")]);
    let session = open(&w, company, prof).await;
    let sale = ring(&w, company, prof, session, &tax, vec![line(item, "1", "100000")]).await.unwrap();
    let (tax_total, grand): (Decimal, Decimal) =
        sqlx::query_as("SELECT tax_total, grand_total FROM pos.pos_invoices WHERE id=$1")
            .bind(sale).fetch_one(&pool).await.unwrap();
    assert_eq!(tax_total, d("12000.00"), "11% + 1% components both apply");
    assert_eq!(grand, d("112000.00"));
}

// TC-3: the port's per-line nets WIN. A globally-rounding tax policy redistributes per-line cents;
// POS adopts those nets verbatim so Σ line nets == header net exactly (the journal balances).
#[tokio::test]
async fn port_net_redistribution_is_adopted_verbatim() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::profile_at_rate(&pool, company, "0").await;
    let tax = tax.with_shift(1); // first line +0.01, last line −0.01
    let session = open(&w, company, prof).await;
    let sale = ring(&w, company, prof, session, &tax, vec![
        line(item, "1", "100000"),
        NewSaleLine { item_id: Uuid::new_v4(), ..line(item, "1", "100000") },
    ]).await.unwrap();
    let nets: Vec<Decimal> = sqlx::query_scalar(
        "SELECT net_amount FROM pos.pos_invoice_items WHERE pos_invoice_id=$1 AND metadata->>'deleted_at' IS NULL ORDER BY net_amount DESC",
    ).bind(sale).fetch_all(&pool).await.unwrap();
    assert_eq!(nets, vec![d("100000.01"), d("99999.99")], "the port's redistributed nets are what persist");
    let header_net: Decimal = sqlx::query_scalar("SELECT net_total FROM pos.pos_invoices WHERE id=$1").bind(sale).fetch_one(&pool).await.unwrap();
    let sum: Decimal = nets.iter().sum();
    assert_eq!(header_net, sum, "Σ line nets == header net (redistribution is conservation-safe)");
}

// TC-4: a port refusal surfaces as the typed TaxRejected error — the register's operator sees the
// tax engine's own code, not a DB failure.
#[tokio::test]
async fn port_refusal_surfaces_as_tax_rejected() {
    struct RefusingTax;
    #[async_trait::async_trait]
    impl backbone_pos::application::service::pos_ports::PosTaxComputePort for RefusingTax {
        async fn compute_document(&self, _req: &backbone_pos::application::service::pos_ports::PosTaxComputeRequest) -> Result<backbone_pos::application::service::pos_ports::PosTaxComputeResult, backbone_pos::application::service::pos_ports::PosRejected> {
            Err(backbone_pos::application::service::pos_ports::PosRejected { code: "period_closed".into(), message: "the tax period is locked".into() })
        }
    }
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, _) = support::profile_at_rate(&pool, company, "0").await;
    let session = open(&w, company, prof).await;
    let e = w.ring_sale(NewSale {
        company_id: company, pos_profile_id: prof, opening_entry_id: session, branch_id: None, customer_id: None,
        pos_table_id: None, discount_id: None,
        receipt_number: uq("R"), posting_at: at(), lines: vec![line(item, "1", "1000")],
    }, &RefusingTax).await.unwrap_err();
    match e {
        PosError::TaxRejected { code, .. } => assert_eq!(code, "period_closed"),
        other => panic!("expected TaxRejected, got {other:?}"),
    }
    // Nothing was written — a refused compute leaves no draft behind.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pos.pos_invoices WHERE opening_entry_id=$1").bind(session).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
}

// TC-5: an unknown template on the profile refuses the whole ring (never a partial tax application) —
// the register names a template the resolver does not know.
#[tokio::test]
async fn unknown_template_refuses_the_ring() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::profile_at_rate(&pool, company, "0").await;
    // A second template id on the profile that the port has no rate for.
    sqlx::query("UPDATE pos.pos_profiles SET tax_template_ids=$2 WHERE id=$1")
        .bind(prof).bind(serde_json::json!([Uuid::new_v4().to_string()])).execute(&pool).await.unwrap();
    let session = open(&w, company, prof).await;
    let e = ring(&w, company, prof, session, &tax, vec![line(item, "1", "1000")]).await.unwrap_err();
    match e {
        PosError::TaxRejected { code, .. } => assert_eq!(code, "unknown_template"),
        other => panic!("expected TaxRejected, got {other:?}"),
    }
}

// TC-6: the derived per-ticket tax rate on the recognition read is computed from the stored totals —
// the retired flat profile column no longer feeds any read path.
#[tokio::test]
async fn derived_tax_rate_replaces_the_profile_column() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let (company, item) = (Uuid::new_v4(), Uuid::new_v4());
    let (prof, tax) = support::profile_at_rate(&pool, company, "0.11").await;
    // A stale flat column must not matter: park a wrong value on it.
    sqlx::query("UPDATE pos.pos_profiles SET tax_rate=0.5 WHERE id=$1").bind(prof).execute(&pool).await.unwrap();
    let session = open(&w, company, prof).await;
    let sale = ring(&w, company, prof, session, &tax, vec![line(item, "1", "100000")]).await.unwrap();
    let (tax_total, rate): (Decimal, Decimal) = sqlx::query_as(
        "SELECT tax_total, ROUND(COALESCE(tax_total / NULLIF(net_total, 0), 0), 6) FROM pos.pos_invoices WHERE id=$1",
    ).bind(sale).fetch_one(&pool).await.unwrap();
    assert_eq!(tax_total, d("11000.00"));
    assert_eq!(rate, d("0.110000"), "the derived rate comes from the stored totals, not the profile column");
}
