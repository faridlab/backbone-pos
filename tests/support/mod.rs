//! Shared fixtures for the hand-written POS behavior tests (user-owned).
//!
//! In-test implementations of the module's outbound ports + DB seeders. The REAL cross-module seams
//! (billing/payment/accounting/inventory) live in `retail_sale_seam.rs`; these fakes are for
//! POS-only behavior: money derivation, offline sync, manager PINs, drawer closes.
//!
//! Every test file that needs them declares `mod support;`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use backbone_pos::application::service::pos_events::{PosEvent, PosEventSink};
use backbone_pos::application::service::pos_manager_pin::SetPin;
use backbone_pos::application::service::pos_ports::{
    BillingPort, CashVarianceAck, CashVarianceDirection, CashVarianceRequest, CreditNoteRequest,
    InvoiceAck, PaymentPort, PosCashVariancePort, PosRejected, PosTaxComponent,
    PosTaxComputePort, PosTaxComputeRequest, PosTaxComputeResult, RefundRequest, ReversalAck,
    SaleInvoiceRequest, SettlementAck, SettlementRequest,
};
use backbone_pos::application::service::pos_write_service::{ManagerAuth, PosWriteService};

pub fn d(s: &str) -> Decimal {
    Decimal::from_str_exact(s).unwrap()
}

pub fn at() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(2026, 7, 5).unwrap().and_hms_opt(9, 0, 0).unwrap()
}

pub fn uq(p: &str) -> String {
    format!("{p}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

pub async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5433/backbone_pos".to_string());
    PgPool::connect(&url).await.expect("connect DB")
}

// ---- register profiles --------------------------------------------------------

/// Seed a register row with the given tax templates (+ optional cash rounding config). The tax RATES
/// live in the [`TestTax`] the test pairs with these ids — the port is the resolver, the profile only
/// names which templates apply.
pub async fn seed_profile(
    pool: &PgPool,
    company: Uuid,
    templates: &[Uuid],
    rounding: Option<(&str, Decimal)>,
) -> Uuid {
    let id = Uuid::new_v4();
    let templates_json = serde_json::Value::Array(
        templates.iter().map(|t| serde_json::Value::String(t.to_string())).collect(),
    );
    match rounding {
        None => {
            sqlx::query(
                r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, tax_template_ids, allow_discount, status)
                   VALUES ($1,$2,'Register 1','IDR',$3,true,'active')"#,
            )
            .bind(id)
            .bind(company)
            .bind(templates_json)
            .execute(pool)
            .await
            .unwrap();
        }
        Some((strategy, unit)) => {
            sqlx::query(
                r#"INSERT INTO pos.pos_profiles (id, company_id, name, currency, tax_template_ids, cash_rounding_strategy, cash_rounding_unit, allow_discount, status)
                   VALUES ($1,$2,'Register 1','IDR',$3,$4::pos_cash_rounding_strategy,$5,true,'active')"#,
            )
            .bind(id)
            .bind(company)
            .bind(templates_json)
            .bind(strategy)
            .bind(unit)
            .execute(pool)
            .await
            .unwrap();
        }
    }
    id
}

/// A register with one template — the tax rate applied to it comes from the returned [`TestTax`].
pub async fn profile_at_rate(pool: &PgPool, company: Uuid, rate: &str) -> (Uuid, TestTax) {
    let template = Uuid::new_v4();
    let profile = seed_profile(pool, company, &[template], None).await;
    (profile, TestTax::with_rate(template, rate))
}

/// A zero-rated register (the non-PKP expression: template present, rate 0).
pub async fn zero_tax_profile(pool: &PgPool, company: Uuid) -> (Uuid, TestTax) {
    profile_at_rate(pool, company, "0").await
}

// ---- the tax port fake ----------------------------------------------------------

/// In-test `PosTaxComputePort`: a flat rate per template id, money-rounded per line. Set
/// `.with_shift(1)` to prove the compute core ADOPTS the port's redistributed per-line nets (first
/// line +0.01, last line −0.01 — the cent-moving behaviour a globally-rounding tax policy exhibits).
pub struct TestTax {
    rates: HashMap<Uuid, Decimal>,
    shift_cents: i64,
}

impl TestTax {
    pub fn with_rate(template: Uuid, rate: &str) -> Self {
        let mut rates = HashMap::new();
        rates.insert(template, d(rate));
        Self { rates, shift_cents: 0 }
    }

    pub fn with_rates(entries: Vec<(Uuid, &str)>) -> Self {
        Self { rates: entries.into_iter().map(|(t, r)| (t, d(r))).collect(), shift_cents: 0 }
    }

    /// Redistribute per-line nets by one cent (first +, last −) — adopted verbatim by the caller.
    pub fn with_shift(mut self, cents: i64) -> Self {
        self.shift_cents = cents;
        self
    }
}

#[async_trait::async_trait]
impl PosTaxComputePort for TestTax {
    async fn compute_document(&self, req: &PosTaxComputeRequest) -> Result<PosTaxComputeResult, PosRejected> {
        let n = req.lines.len();
        let mut components = Vec::with_capacity(n);
        let mut net_amounts = Vec::with_capacity(n);
        let mut tax_total = Decimal::ZERO;
        for (i, l) in req.lines.iter().enumerate() {
            let rate = *self.rates.get(&l.template_id).ok_or_else(|| PosRejected {
                code: "unknown_template".into(),
                message: format!("no rate mapped for template {}", l.template_id),
            })?;
            let tax = (l.net_amount * rate).round_dp(2);
            tax_total += tax;
            components.push(PosTaxComponent {
                line_ref: l.line_ref,
                template_id: l.template_id,
                account_id: None,
                real_account_id: None,
                rate,
                tax_amount: tax,
                description: Some(format!("test tax {}", rate)),
            });
            if self.shift_cents != 0 && n >= 2 {
                let cents = if i == 0 {
                    self.shift_cents
                } else if i + 1 == n {
                    -self.shift_cents
                } else {
                    0
                };
                if cents != 0 {
                    net_amounts
                        .push((l.line_ref, (l.net_amount + Decimal::from(cents) / Decimal::from(100)).round_dp(2)));
                }
            }
        }
        Ok(PosTaxComputeResult {
            net_amounts,
            components,
            excluded_total: Decimal::ZERO,
            tax_total: tax_total.round_dp(2),
            included_total: Decimal::ZERO,
        })
    }
}

// ---- the variance port fake -----------------------------------------------------

/// In-test `PosCashVariancePort`: records every booking request and replays a STABLE journal id per
/// session (idempotency by `opening_entry_id`, exactly as the real accounting adapter must).
#[derive(Default, Clone)]
pub struct RecordingVariance {
    pub booked: Arc<Mutex<Vec<(CashVarianceRequest, Uuid)>>>,
}

#[async_trait::async_trait]
impl PosCashVariancePort for RecordingVariance {
    async fn book_cash_variance(&self, req: &CashVarianceRequest) -> Result<CashVarianceAck, PosRejected> {
        let mut booked = self.booked.lock().unwrap();
        if let Some((_, journal_id)) = booked.iter().find(|(r, _)| r.opening_entry_id == req.opening_entry_id) {
            return Ok(CashVarianceAck { journal_id: *journal_id });
        }
        let journal_id = Uuid::new_v4();
        booked.push((req.clone(), journal_id));
        Ok(CashVarianceAck { journal_id })
    }
}

impl RecordingVariance {
    /// (amount, direction) of the variance booked for one session, if any.
    pub fn booking_for(&self, opening_entry_id: Uuid) -> Option<(Decimal, CashVarianceDirection)> {
        self.booked
            .lock()
            .unwrap()
            .iter()
            .find(|(r, _)| r.opening_entry_id == opening_entry_id)
            .map(|(r, _)| (r.amount, r.direction))
    }
}

// ---- billing / payment fakes (POS-only tests) -----------------------------------

#[derive(Default, Clone)]
pub struct StubBilling {
    pub raised: Arc<Mutex<usize>>,
    pub invoice: Uuid,
}

#[async_trait::async_trait]
impl BillingPort for StubBilling {
    async fn raise_and_post(&self, _req: &SaleInvoiceRequest) -> Result<InvoiceAck, PosRejected> {
        *self.raised.lock().unwrap() += 1;
        Ok(InvoiceAck { invoice_id: self.invoice, journal_id: Uuid::new_v4(), grand_total: d("0") })
    }
    async fn credit_note(&self, _req: &CreditNoteRequest) -> Result<ReversalAck, PosRejected> {
        Ok(ReversalAck { journal_id: Uuid::new_v4() })
    }
}

#[derive(Default, Clone)]
pub struct StubPayment;

#[async_trait::async_trait]
impl PaymentPort for StubPayment {
    async fn settle(&self, _req: &SettlementRequest) -> Result<SettlementAck, PosRejected> {
        Ok(SettlementAck { payment_id: Uuid::new_v4(), journal_id: Uuid::new_v4() })
    }
    async fn refund(&self, _req: &RefundRequest) -> Result<ReversalAck, PosRejected> {
        Ok(ReversalAck { journal_id: Uuid::new_v4() })
    }
}

// ---- events + manager PIN fixtures ----------------------------------------------

#[derive(Default, Clone)]
pub struct Recorder {
    pub events: Arc<Mutex<Vec<PosEvent>>>,
}

impl PosEventSink for Recorder {
    fn publish(&self, e: PosEvent) {
        self.events.lock().unwrap().push(e);
    }
}

/// Give a fresh manager a PIN (the bootstrap path — no prior credential exists) and return the auth a
/// privileged verb carries.
pub async fn manager_with_pin(w: &PosWriteService, company: Uuid, pin: &str) -> ManagerAuth {
    let employee = Uuid::new_v4();
    w.set_pin(SetPin {
        company_id: company,
        employee_party_id: employee,
        new_pin: pin.to_string(),
        current: None,
        source_ip: None,
    })
    .await
    .expect("bootstrap set_pin");
    ManagerAuth { employee_party_id: employee, pin: pin.to_string() }
}
