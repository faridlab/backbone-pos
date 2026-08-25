use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use super::AuditMetadata;

/// Strongly-typed ID for PosManagerPin
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PosManagerPinId(pub Uuid);

impl PosManagerPinId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PosManagerPinId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PosManagerPinId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PosManagerPinId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PosManagerPinId> for Uuid {
    fn from(id: PosManagerPinId) -> Self { id.0 }
}

impl AsRef<Uuid> for PosManagerPinId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PosManagerPinId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PosManagerPin {
    pub id: Uuid,
    pub company_id: Uuid,
    pub employee_party_id: Uuid,
    pub pin_hash: String,
    pub failed_attempts: i32,
    pub locked_until: Option<DateTime<Utc>>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_attempt_ip: Option<String>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PosManagerPin {
    /// Create a builder for PosManagerPin
    pub fn builder() -> PosManagerPinBuilder {
        <PosManagerPinBuilder as Default>::default()
    }

    /// Create a new PosManagerPin with required fields
    pub fn new(company_id: Uuid, employee_party_id: Uuid, pin_hash: String, failed_attempts: i32) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            employee_party_id,
            pin_hash,
            failed_attempts,
            locked_until: None,
            last_attempt_at: None,
            last_attempt_ip: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PosManagerPinId {
        PosManagerPinId(self.id)
    }

    /// Get when this entity was created
    pub fn created_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.created_at.as_ref()
    }

    /// Get when this entity was last updated
    pub fn updated_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.updated_at.as_ref()
    }

    /// Check if this entity is soft deleted
    pub fn is_deleted(&self) -> bool {
        self.metadata.deleted_at.is_some()
    }

    /// Check if this entity is active (not deleted)
    pub fn is_active(&self) -> bool {
        self.metadata.deleted_at.is_none()
    }

    /// Get when this entity was deleted
    pub fn deleted_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.deleted_at.as_ref()
    }

    /// Get who created this entity
    pub fn created_by(&self) -> Option<&Uuid> {
        self.metadata.created_by.as_ref()
    }

    /// Get who last updated this entity
    pub fn updated_by(&self) -> Option<&Uuid> {
        self.metadata.updated_by.as_ref()
    }

    /// Get who deleted this entity
    pub fn deleted_by(&self) -> Option<&Uuid> {
        self.metadata.deleted_by.as_ref()
    }


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the locked_until field (chainable)
    pub fn with_locked_until(mut self, value: DateTime<Utc>) -> Self {
        self.locked_until = Some(value);
        self
    }

    /// Set the last_attempt_at field (chainable)
    pub fn with_last_attempt_at(mut self, value: DateTime<Utc>) -> Self {
        self.last_attempt_at = Some(value);
        self
    }

    /// Set the last_attempt_ip field (chainable)
    pub fn with_last_attempt_ip(mut self, value: String) -> Self {
        self.last_attempt_ip = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "company_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.company_id = v; }
                }
                "employee_party_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.employee_party_id = v; }
                }
                "pin_hash" => {
                    if let Ok(v) = serde_json::from_value(value) { self.pin_hash = v; }
                }
                "failed_attempts" => {
                    if let Ok(v) = serde_json::from_value(value) { self.failed_attempts = v; }
                }
                "locked_until" => {
                    if let Ok(v) = serde_json::from_value(value) { self.locked_until = v; }
                }
                "last_attempt_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.last_attempt_at = v; }
                }
                "last_attempt_ip" => {
                    if let Ok(v) = serde_json::from_value(value) { self.last_attempt_ip = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PosManagerPin {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PosManagerPin"
    }
}

impl backbone_core::PersistentEntity for PosManagerPin {
    fn entity_id(&self) -> String {
        self.id.to_string()
    }
    fn set_entity_id(&mut self, id: String) {
        if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
            self.id = uuid;
        }
    }
    fn created_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.created_at
    }
    fn set_created_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.created_at = Some(ts);
    }
    fn updated_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.updated_at
    }
    fn set_updated_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.updated_at = Some(ts);
    }
    fn deleted_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.deleted_at
    }
    fn set_deleted_at(&mut self, ts: Option<chrono::DateTime<chrono::Utc>>) {
        self.metadata.deleted_at = ts;
    }
}

impl backbone_orm::EntityRepoMeta for PosManagerPin {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("employee_party_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["pin_hash"]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for PosManagerPin entity
///
/// Provides a fluent API for constructing PosManagerPin instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PosManagerPinBuilder {
    company_id: Option<Uuid>,
    employee_party_id: Option<Uuid>,
    pin_hash: Option<String>,
    failed_attempts: Option<i32>,
    locked_until: Option<DateTime<Utc>>,
    last_attempt_at: Option<DateTime<Utc>>,
    last_attempt_ip: Option<String>,
}

impl PosManagerPinBuilder {
    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Set the employee_party_id field (required)
    pub fn employee_party_id(mut self, value: Uuid) -> Self {
        self.employee_party_id = Some(value);
        self
    }

    /// Set the pin_hash field (required)
    pub fn pin_hash(mut self, value: String) -> Self {
        self.pin_hash = Some(value);
        self
    }

    /// Set the failed_attempts field (default: `0`)
    pub fn failed_attempts(mut self, value: i32) -> Self {
        self.failed_attempts = Some(value);
        self
    }

    /// Set the locked_until field (optional)
    pub fn locked_until(mut self, value: DateTime<Utc>) -> Self {
        self.locked_until = Some(value);
        self
    }

    /// Set the last_attempt_at field (optional)
    pub fn last_attempt_at(mut self, value: DateTime<Utc>) -> Self {
        self.last_attempt_at = Some(value);
        self
    }

    /// Set the last_attempt_ip field (optional)
    pub fn last_attempt_ip(mut self, value: String) -> Self {
        self.last_attempt_ip = Some(value);
        self
    }

    /// Build the PosManagerPin entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PosManagerPin, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let employee_party_id = self.employee_party_id.ok_or_else(|| "employee_party_id is required".to_string())?;
        let pin_hash = self.pin_hash.ok_or_else(|| "pin_hash is required".to_string())?;

        Ok(PosManagerPin {
            id: Uuid::new_v4(),
            company_id,
            employee_party_id,
            pin_hash,
            failed_attempts: self.failed_attempts.unwrap_or(0),
            locked_until: self.locked_until,
            last_attempt_at: self.last_attempt_at,
            last_attempt_ip: self.last_attempt_ip,
            metadata: AuditMetadata::default(),
        })
    }
}
