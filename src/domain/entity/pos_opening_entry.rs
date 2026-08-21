use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::PosSessionStatus;
use super::AuditMetadata;

/// Strongly-typed ID for PosOpeningEntry
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PosOpeningEntryId(pub Uuid);

impl PosOpeningEntryId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PosOpeningEntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PosOpeningEntryId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PosOpeningEntryId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PosOpeningEntryId> for Uuid {
    fn from(id: PosOpeningEntryId) -> Self { id.0 }
}

impl AsRef<Uuid> for PosOpeningEntryId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PosOpeningEntryId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PosOpeningEntry {
    pub id: Uuid,
    pub company_id: Uuid,
    pub pos_profile_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub cashier_party_id: Uuid,
    pub opened_at: DateTime<Utc>,
    pub opening_balances: Option<serde_json::Value>,
    pub status: PosSessionStatus,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PosOpeningEntry {
    /// Create a builder for PosOpeningEntry
    pub fn builder() -> PosOpeningEntryBuilder {
        <PosOpeningEntryBuilder as Default>::default()
    }

    /// Create a new PosOpeningEntry with required fields
    pub fn new(company_id: Uuid, pos_profile_id: Uuid, cashier_party_id: Uuid, opened_at: DateTime<Utc>, status: PosSessionStatus) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            pos_profile_id,
            branch_id: None,
            cashier_party_id,
            opened_at,
            opening_balances: None,
            status,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PosOpeningEntryId {
        PosOpeningEntryId(self.id)
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

    /// Get the current status
    pub fn status(&self) -> &PosSessionStatus {
        &self.status
    }


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the branch_id field (chainable)
    pub fn with_branch_id(mut self, value: Uuid) -> Self {
        self.branch_id = Some(value);
        self
    }

    /// Set the opening_balances field (chainable)
    pub fn with_opening_balances(mut self, value: serde_json::Value) -> Self {
        self.opening_balances = Some(value);
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
                "pos_profile_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.pos_profile_id = v; }
                }
                "branch_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.branch_id = v; }
                }
                "cashier_party_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.cashier_party_id = v; }
                }
                "opened_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.opened_at = v; }
                }
                "opening_balances" => {
                    if let Ok(v) = serde_json::from_value(value) { self.opening_balances = v; }
                }
                "status" => {
                    if let Ok(v) = serde_json::from_value(value) { self.status = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PosOpeningEntry {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PosOpeningEntry"
    }
}

impl backbone_core::PersistentEntity for PosOpeningEntry {
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

impl backbone_orm::EntityRepoMeta for PosOpeningEntry {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("pos_profile_id".to_string(), "uuid".to_string());
        m.insert("branch_id".to_string(), "uuid".to_string());
        m.insert("cashier_party_id".to_string(), "uuid".to_string());
        m.insert("status".to_string(), "pos_session_status".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for PosOpeningEntry entity
///
/// Provides a fluent API for constructing PosOpeningEntry instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PosOpeningEntryBuilder {
    company_id: Option<Uuid>,
    pos_profile_id: Option<Uuid>,
    branch_id: Option<Uuid>,
    cashier_party_id: Option<Uuid>,
    opened_at: Option<DateTime<Utc>>,
    opening_balances: Option<serde_json::Value>,
    status: Option<PosSessionStatus>,
}

impl PosOpeningEntryBuilder {
    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Set the pos_profile_id field (required)
    pub fn pos_profile_id(mut self, value: Uuid) -> Self {
        self.pos_profile_id = Some(value);
        self
    }

    /// Set the branch_id field (optional)
    pub fn branch_id(mut self, value: Uuid) -> Self {
        self.branch_id = Some(value);
        self
    }

    /// Set the cashier_party_id field (required)
    pub fn cashier_party_id(mut self, value: Uuid) -> Self {
        self.cashier_party_id = Some(value);
        self
    }

    /// Set the opened_at field (required)
    pub fn opened_at(mut self, value: DateTime<Utc>) -> Self {
        self.opened_at = Some(value);
        self
    }

    /// Set the opening_balances field (optional)
    pub fn opening_balances(mut self, value: serde_json::Value) -> Self {
        self.opening_balances = Some(value);
        self
    }

    /// Set the status field (default: `PosSessionStatus::default()`)
    pub fn status(mut self, value: PosSessionStatus) -> Self {
        self.status = Some(value);
        self
    }

    /// Build the PosOpeningEntry entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PosOpeningEntry, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let pos_profile_id = self.pos_profile_id.ok_or_else(|| "pos_profile_id is required".to_string())?;
        let cashier_party_id = self.cashier_party_id.ok_or_else(|| "cashier_party_id is required".to_string())?;
        let opened_at = self.opened_at.ok_or_else(|| "opened_at is required".to_string())?;

        Ok(PosOpeningEntry {
            id: Uuid::new_v4(),
            company_id,
            pos_profile_id,
            branch_id: self.branch_id,
            cashier_party_id,
            opened_at,
            opening_balances: self.opening_balances,
            status: self.status.unwrap_or_default(),
            metadata: AuditMetadata::default(),
        })
    }
}
