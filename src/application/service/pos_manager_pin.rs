//! The manager-PIN credential path (hand-authored, user-owned).
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`]. A manager
//! PIN is the register's Tier-B credential (PSX-4): a short numeric code whose ARGON2 HASH lives on
//! `pos.pos_manager_pins`, one live PIN per manager per company. The hash NEVER leaves the server —
//! not into a response, not into an event, not into a log line.
//!
//! **Privileged verbs verify on every call.** There is no session, no token, no "manager mode" a
//! client can assert: each privileged mutation carries a [`ManagerAuth`] (employee + PIN) and the
//! service re-verifies it against the hash before writing anything. The privileged surface of this
//! module:
//!
//! - [`Self::set_pin`] / [`Self::verify_pin`] — the credential verbs themselves.
//! - [`super::pos_drawer::PosWriteService::close_session`] — closing the till books a GL variance.
//! - [`super::pos_sync::PosWriteService::sync_from_ui`] — only when the replay is a refund
//!   (`refund_of_client_uuid` set); a plain sale replay is a cashier action.
//!
//! Anti-abuse is two-layered and entirely CONFIG-DRIVEN ([`PinPolicy`] — env overrides, short
//! defaults): a per-manager consecutive-failure counter with lockout (persisted, so a restart does
//! not reset an attack), and a per-source-address throttle ring in front of it (process-local).
//!
//! Per the module's 4-layer rule this file holds no SQL — the credential statements live on
//! `PosManagerPinRepository`.

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use uuid::Uuid;

use backbone_orm::company_scope;

use super::pos_write_service::{ManagerAuth, PosError, PosWriteService};

/// A `set_pin` request: the manager getting a credential + the new PIN + the proof of authority.
#[derive(Debug, Clone)]
pub struct SetPin {
    pub company_id: Uuid,
    pub employee_party_id: Uuid,
    /// Digits only; length must fall inside the policy window.
    pub new_pin: String,
    /// Proof of authority to set: the SAME manager's current PIN (self-change), ANOTHER manager's
    /// verified PIN (supervised change), or `None` — allowed only while the company has no live PIN
    /// at all (the bootstrap: the very first credential cannot demand a credential).
    pub current: Option<ManagerAuth>,
    pub source_ip: Option<String>,
}

impl PosWriteService {
    /// Set (or replace) a manager's PIN. Hashes with argon2 server-side and persists ONLY the hash;
    /// setting a new PIN clears any lockout and failure history (the administrative unlock).
    pub async fn set_pin(&self, s: SetPin) -> Result<(), PosError> {
        // Strength first — a weak PIN never reaches the hash (and never burns the authority proof).
        validate_pin_strength(&s.new_pin, &self.pin_policy)?;
        let company = s.company_id;
        company_scope::with_company_scope(Some(company), async move {
            // Authority: once ANY credential exists at the company, changing one requires proof —
            // the manager's own current PIN, or another live manager's.
            let bootstrapping = !self.pins.any_live_pin_exists(&self.db_pool, company).await?;
            if !bootstrapping {
                let proof = s.current.as_ref().ok_or(PosError::ManagerAuthRequired)?;
                self.verify_manager_internal(company, proof, s.source_ip.as_deref()).await?;
            }
            let salt = SaltString::generate(&mut OsRng);
            let hash = Argon2::default()
                .hash_password(s.new_pin.as_bytes(), &salt)
                .map_err(|e| PosError::Db(sqlx::Error::Protocol(format!("pin hash: {e}"))))?;
            self.pins
                .upsert_hash(&self.db_pool, company, s.employee_party_id, &hash.to_string(), chrono::Utc::now(), s.source_ip.as_deref())
                .await?;
            Ok(())
        })
        .await
    }

    /// Verify a manager's PIN — the gate every privileged mutation calls. Fails CLOSED on every
    /// abuse signal: unknown manager (`PinNotFound`), wrong PIN (`PinInvalid`), locked
    /// (`PinLocked`), source over its throttle budget (`PinThrottled`). Succeeds by clearing the
    /// failure counter. State changes ride the persisted counters, so a process restart never
    /// resets an in-progress attack.
    pub async fn verify_pin(
        &self,
        company_id: Uuid,
        employee_party_id: Uuid,
        pin: &str,
        source_ip: Option<&str>,
    ) -> Result<(), PosError> {
        self.verify_manager_internal(
            company_id,
            &ManagerAuth { employee_party_id, pin: pin.to_string() },
            source_ip,
        )
        .await
    }

    /// The shared verify path (also what privileged verbs call through [`ManagerAuth`]).
    pub(super) async fn verify_manager_internal(
        &self,
        company_id: Uuid,
        auth: &ManagerAuth,
        source_ip: Option<&str>,
    ) -> Result<(), PosError> {
        // Per-source throttle FIRST — a throttled source does not get to touch the manager's
        // counter at all (the counter is per-manager; the ring is per-address, so one address
        // hammering different managers still gets fenced).
        if let Some(ip) = source_ip {
            if !self.take_ip_budget(ip) {
                return Err(PosError::PinThrottled);
            }
        }
        company_scope::with_company_scope(Some(company_id), async move {
            let cred = self.pins
                .fetch_credential(&self.db_pool, company_id, auth.employee_party_id)
                .await?
                .ok_or(PosError::PinNotFound)?;
            let now = chrono::Utc::now();
            if let Some(until) = cred.locked_until {
                if until > now {
                    return Err(PosError::PinLocked { until });
                }
            }
            let parsed = match PasswordHash::new(&cred.pin_hash) {
                Ok(h) => h,
                // An unreadable hash — a corrupt row, or a credential-blind CRUD create that landed on
                // the non-verifying placeholder — is not a server fault: it is a credential that cannot
                // authenticate anyone. Fail closed as a wrong-PIN (403), never a 500.
                Err(_) => return Err(PosError::PinInvalid),
            };
            if Argon2::default().verify_password(auth.pin.as_bytes(), &parsed).is_ok() {
                self.pins.record_success(&self.db_pool, cred.id, now, source_ip).await?;
                Ok(())
            } else {
                let lockout_until = now + chrono::Duration::seconds(self.pin_policy.lockout_secs);
                self.pins
                    .record_failure(&self.db_pool, cred.id, self.pin_policy.max_attempts, lockout_until, now, source_ip)
                    .await?;
                // Report the lock when THIS failure crossed the threshold — the manager sees the
                // unlock instant instead of a bare "wrong PIN" they cannot recover from.
                if cred.failed_attempts + 1 >= self.pin_policy.max_attempts as i32 {
                    Err(PosError::PinLocked { until: lockout_until })
                } else {
                    Err(PosError::PinInvalid)
                }
            }
        })
        .await
    }

    /// Consume one verification attempt from a source address's rolling budget. `false` = over
    /// budget inside the window. Pure sync (Mutex never held across an `.await`).
    fn take_ip_budget(&self, ip: &str) -> bool {
        let key = format!("pin:{ip}");
        let now = chrono::Utc::now().timestamp();
        // A poisoned ring means some thread panicked mid-update; the counts are still coherent
        // (poisoning marks ownership doubt, not corrupt data), so recover the inner map rather
        // than panicking this request too. The persisted per-manager counters remain the real
        // credential fence either way — this ring is a process-local pre-filter.
        let mut ring = self.ip_attempts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // Read the window state first (release the shared borrow), then decide.
        let in_window = match ring.get(&key) {
            Some((window_start, count)) if now - *window_start < self.pin_policy.ip_window_secs => Some(*count),
            _ => None,
        };
        match in_window {
            // Over budget inside the window: fenced.
            Some(count) if count >= self.pin_policy.ip_max_attempts => false,
            // Inside the window with budget left: consume one. The `if let` is belt-and-braces:
            // the entry was just read, and a missed increment fails SAFE (this attempt is simply
            // not counted — the persisted counters still fence the credential).
            Some(_) => {
                if let Some((_, count)) = ring.get_mut(&key) {
                    *count += 1;
                }
                true
            }
            // No entry, or the previous window expired: start a fresh window with this attempt.
            None => {
                ring.insert(key, (now, 1));
                true
            }
        }
    }
}

/// Digits-only + policy length window. The reason string is static so the error wire stays stable.
fn validate_pin_strength(pin: &str, policy: &super::pos_write_service::PinPolicy) -> Result<(), PosError> {
    if pin.is_empty() || !pin.bytes().all(|b| b.is_ascii_digit()) {
        return Err(PosError::WeakPin("pin must be digits only"));
    }
    let n = pin.len();
    if n < policy.min_digits {
        return Err(PosError::WeakPin("pin is too short"));
    }
    if n > policy.max_digits {
        return Err(PosError::WeakPin("pin is too long"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::service::pos_write_service::PinPolicy;

    fn policy() -> PinPolicy {
        PinPolicy { max_attempts: 3, lockout_secs: 5, min_digits: 4, max_digits: 6, ip_window_secs: 10, ip_max_attempts: 4 }
    }

    #[test]
    fn strength_rejects_short_long_and_nondigits() {
        let p = policy();
        assert!(matches!(validate_pin_strength("123", &p), Err(PosError::WeakPin(_))));
        assert!(matches!(validate_pin_strength("1234567", &p), Err(PosError::WeakPin(_))));
        assert!(matches!(validate_pin_strength("12ab", &p), Err(PosError::WeakPin(_))));
        assert!(validate_pin_strength("4321", &p).is_ok());
    }

    #[test]
    fn env_policy_falls_back_to_defaults_on_garbage() {
        // Parsing is forgiving by design: a bad override must fall back, never panic or zero a window.
        std::env::set_var("POS_PIN_TEST_SENTINEL", "not-a-number");
        let d = PinPolicy::default();
        let p = PinPolicy::from_env();
        assert_eq!(p.max_attempts, d.max_attempts);
        assert_eq!(p.lockout_secs, d.lockout_secs);
    }
}
