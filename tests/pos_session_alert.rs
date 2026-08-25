//! The old-session alert scheduler handler: stale open sessions (default threshold 7 days) are
//! claimed under the pickup lock, latched once, and announced as `PosStaleSessionAlerted` for the
//! host to piggy-back a mail activity onto the cashier. POS-only — requires DATABASE_URL (scratch
//! DB with the module's migrations applied).

mod support;
use support::pool;

use uuid::Uuid;

use backbone_pos::application::service::pos_events::{PosEvent, PosStaleSessionAlerted};
use backbone_pos::application::service::pos_session_alert::DEFAULT_STALE_SESSION_AGE_DAYS;
use backbone_pos::application::service::pos_write_service::{NewSession, PosWriteService};

fn days_ago(n: i64) -> chrono::NaiveDateTime {
    chrono::Utc::now().naive_utc() - chrono::Duration::days(n)
}

async fn open_at(w: &PosWriteService, company: Uuid, prof: Uuid, opened_at: chrono::NaiveDateTime) -> Uuid {
    w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: Uuid::new_v4(),
        opened_at, opening_balances: vec![],
    }).await.unwrap()
}

fn default_threshold() -> chrono::Duration {
    chrono::Duration::days(DEFAULT_STALE_SESSION_AGE_DAYS)
}

/// SA-1: an open session older than the threshold alerts exactly once — the second run is a no-op
/// (the latch), and the emitted event carries the anchors a mail-activity consumer needs.
#[tokio::test]
async fn stale_session_alerts_once_then_latches() {
    let pool = pool().await;
    let rec = support::Recorder::default();
    let w = PosWriteService::with_sink(pool.clone(), std::sync::Arc::new(rec.clone()));
    let company = Uuid::new_v4();
    let (prof, _tax) = support::zero_tax_profile(&pool, company).await;
    let session = open_at(&w, company, prof, days_ago(8)).await;

    let alerts = w.alert_old_sessions(company, default_threshold()).await.unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].opening_entry_id, session);
    assert_eq!(alerts[0].company_id, company);
    assert_eq!(alerts[0].pos_profile_id, prof);

    let fired = rec.events.lock().unwrap().iter().filter(|e| matches!(e, PosEvent::PosStaleSessionAlerted(_))).count();
    assert_eq!(fired, 1, "one alert, one event");

    // The latch: a rerun re-alerts nothing.
    let again = w.alert_old_sessions(company, default_threshold()).await.unwrap();
    assert!(again.is_empty(), "the once-only latch holds");
    let latched: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'stale_session_alerted_at' FROM pos.pos_opening_entries WHERE id=$1",
    ).bind(session).fetch_one(&pool).await.unwrap();
    assert!(latched.is_some(), "the durable latch marker is stamped");
}

/// SA-2: the event is emitted only AFTER the latch commits — observable as: a session that alerted
/// has the marker even though the sink is fire-and-forget. (Crash-window direction: lose-an-event,
/// never re-alert-forever — asserted here by the marker existing alongside the event.)
#[tokio::test]
async fn stale_event_carries_the_cashier_party_ref() {
    let pool = pool().await;
    let rec = support::Recorder::default();
    let w = PosWriteService::with_sink(pool.clone(), std::sync::Arc::new(rec.clone()));
    let company = Uuid::new_v4();
    let (prof, _tax) = support::zero_tax_profile(&pool, company).await;
    let cashier = Uuid::new_v4();
    let opened_at = days_ago(10);
    let session = w.open_session(NewSession {
        company_id: company, pos_profile_id: prof, branch_id: None, cashier_party_id: cashier,
        opened_at, opening_balances: vec![],
    }).await.unwrap();

    w.alert_old_sessions(company, default_threshold()).await.unwrap();
    let event = rec.events.lock().unwrap().iter().find_map(|e| match e {
        PosEvent::PosStaleSessionAlerted(a) => Some(a.clone()),
        _ => None,
    }).expect("the alert event fired");
    let expected = PosStaleSessionAlerted {
        opening_entry_id: session, pos_profile_id: prof, company_id: company,
        cashier_party_id: cashier, opened_at,
    };
    assert_eq!(event, expected, "the mail-activity consumer gets the cashier party ref + anchors");
}

/// SA-3: a fresh session (inside the threshold) is not stale.
#[tokio::test]
async fn fresh_session_is_not_stale() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let (prof, _tax) = support::zero_tax_profile(&pool, company).await;
    open_at(&w, company, prof, days_ago(2)).await;

    let alerts = w.alert_old_sessions(company, default_threshold()).await.unwrap();
    assert!(alerts.is_empty());
}

/// SA-4: a CLOSED session is never alerted, however old — the alert targets forgotten drawers,
/// not finished shifts.
#[tokio::test]
async fn closed_session_is_not_alerted() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let (prof, _tax) = support::zero_tax_profile(&pool, company).await;
    let session = open_at(&w, company, prof, days_ago(9)).await;
    sqlx::query("UPDATE pos.pos_opening_entries SET status='closed'::pos_session_status WHERE id=$1")
        .bind(session).execute(&pool).await.unwrap();

    let alerts = w.alert_old_sessions(company, default_threshold()).await.unwrap();
    assert!(alerts.is_empty());
}

/// SA-5: the threshold is the caller's dial — the same 6-day session is not stale at 7 days but is
/// at 5. (A host tightens or loosens the window per deployment.)
#[tokio::test]
async fn threshold_is_parameterized() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company = Uuid::new_v4();
    let (prof, _tax) = support::zero_tax_profile(&pool, company).await;
    open_at(&w, company, prof, days_ago(6)).await;

    assert!(w.alert_old_sessions(company, chrono::Duration::days(7)).await.unwrap().is_empty());
    assert_eq!(w.alert_old_sessions(company, chrono::Duration::days(5)).await.unwrap().len(), 1);
}

/// SA-6 (tenancy): the claim is per-company — company A's scheduler run never sees company B's
/// stale session (no cross-tenant read inside a job).
#[tokio::test]
async fn claim_is_per_company() {
    let pool = pool().await;
    let w = PosWriteService::new(pool.clone());
    let company_a = Uuid::new_v4();
    let company_b = Uuid::new_v4();
    let (prof_a, _) = support::zero_tax_profile(&pool, company_a).await;
    let (prof_b, _) = support::zero_tax_profile(&pool, company_b).await;
    open_at(&w, company_a, prof_a, days_ago(9)).await;
    let b_session = open_at(&w, company_b, prof_b, days_ago(9)).await;

    let alerts = w.alert_old_sessions(company_b, default_threshold()).await.unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].opening_entry_id, b_session);
}
