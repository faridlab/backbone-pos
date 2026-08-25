//! Old-session alert (hand-authored, user-owned): the handler behind the module's ONE scheduled job.
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`]. Open cashier
//! sessions that outlive the threshold (7 days by default — a session that old is a forgotten
//! drawer, not a long shift) get claimed, latched, and announced.
//!
//! Shape decisions (the scheduler declaration in `schema/hooks/index.hook.yaml` records the same):
//!
//! - **Per-company handler.** The host enumerates its companies and calls this once per company, so
//!   the module itself never does a cross-tenant read (ADR-0008) — the fence stays meaningful even
//!   inside a job.
//! - **Once-only latch.** The claim stamps `stale_session_alerted_at` into the session's `metadata`
//!   in the SAME transaction that claims it (`commit_policy: single_transaction`), so a daily
//!   scheduler never re-nags the same session; events are emitted only after that commit.
//! - **Pickup lock.** The claim reads `FOR UPDATE SKIP LOCKED` (ADR-0020 §4), so two concurrent
//!   hosts draining the same schedule each take disjoint sessions instead of colliding.
//! - **Mail is a logical link, not a Cargo edge.** The emitted [`PosStaleSessionAlerted`] carries the
//!   cashier as a party ref; the COMPOSING SERVICE subscribes and schedules the "session open too
//!   long" activity on that cashier. This module depends on no mail crate.
//!
//! Per the module's 4-layer rule this file holds no SQL — the claim/stamp live on
//! `PosOpeningEntryRepository`.

use uuid::Uuid;

use backbone_orm::company_scope;

use crate::infrastructure::persistence::StaleSessionRow;

use super::pos_events::{PosEvent, PosStaleSessionAlerted};
use super::pos_write_service::{PosError, PosWriteService};

/// A session older than this is stale. 7 days is the default; the host may pass its own threshold.
pub const DEFAULT_STALE_SESSION_AGE_DAYS: i64 = 7;

/// Upper bound on sessions alerted in one run — the job stays bounded on a tenant that somehow
/// accumulated years of open drawers. The next run picks up what this one skipped (the latch is only
/// stamped on alerted sessions, never on skipped ones).
pub const STALE_SESSION_BATCH_LIMIT: i64 = 500;

impl PosWriteService {
    /// Alert this company's stale open sessions. Returns the alerts that fired (empty = nothing
    /// stale or everything already latched). Idempotent per session: a rerun re-alerts nothing.
    pub async fn alert_old_sessions(
        &self,
        company_id: Uuid,
        older_than: chrono::Duration,
    ) -> Result<Vec<PosStaleSessionAlerted>, PosError> {
        let cutoff = chrono::Utc::now().naive_utc() - older_than;
        // RLS scope (ADR-0008): the whole claim + latch runs inside the company scope, on one
        // connection, as one transaction — exactly the pattern the write verbs follow.
        let claimed = company_scope::with_company_scope(Some(company_id), async move {
            let mut tx = self.db_pool.begin().await?;
            // Bind the company on the transaction's connection (the write-verb pattern): under a
            // non-bypassing role the RLS fence is what makes the claim read anything at all; the
            // explicit `company_id = $1` filter below stays as defense-in-depth.
            company_scope::bind_current_company(&mut tx).await?;
            let claimed = self
                .openings
                .claim_stale_sessions(&mut tx, company_id, cutoff, STALE_SESSION_BATCH_LIMIT)
                .await?;
            if claimed.is_empty() {
                tx.rollback().await?;
                return Ok::<Vec<StaleSessionRow>, PosError>(Vec::new());
            }
            let at = chrono::Utc::now();
            let ids: Vec<Uuid> = claimed.iter().map(|s| s.opening_entry_id).collect();
            self.openings.mark_stale_alerted(&mut tx, &ids, at).await?;
            tx.commit().await?;
            Ok(claimed)
        })
        .await?;

        // Emit only after the latch committed — a crash between commit and emit loses one alert run
        // for that session (acceptable: the latch is the durable record; the host can re-derive), but
        // the reverse order would re-alert forever.
        let alerts = claimed
            .into_iter()
            .map(|s| PosStaleSessionAlerted {
                opening_entry_id: s.opening_entry_id,
                pos_profile_id: s.pos_profile_id,
                company_id,
                cashier_party_id: s.cashier_party_id,
                opened_at: s.opened_at,
            })
            .collect::<Vec<_>>();
        for a in &alerts {
            self.sink.publish(PosEvent::PosStaleSessionAlerted(a.clone()));
        }
        Ok(alerts)
    }
}
