use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::PosClosingStatus;
use super::AuditMetadata;

/// Strongly-typed ID for PosClosingEntry
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PosClosingEntryId(pub Uuid);

impl PosClosingEntryId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PosClosingEntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PosClosingEntryId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PosClosingEntryId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PosClosingEntryId> for Uuid {
    fn from(id: PosClosingEntryId) -> Self { id.0 }
}

impl AsRef<Uuid> for PosClosingEntryId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PosClosingEntryId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PosClosingEntry {
    pub id: Uuid,
    pub company_id: Uuid,
    pub pos_profile_id: Uuid,
    pub opening_entry_id: Uuid,
    pub closed_at: DateTime<Utc>,
    pub cashier_party_id: Uuid,
    pub totals_by_method: Option<serde_json::Value>,
    pub grand_total: Decimal,
    pub invoice_count: i32,
    pub difference_total: Decimal,
    pub status: PosClosingStatus,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PosClosingEntry {
    /// Create a builder for PosClosingEntry
    pub fn builder() -> PosClosingEntryBuilder {
        PosClosingEntryBuilder::default()
    }

    /// Create a new PosClosingEntry with required fields
    pub fn new(company_id: Uuid, pos_profile_id: Uuid, opening_entry_id: Uuid, closed_at: DateTime<Utc>, cashier_party_id: Uuid, grand_total: Decimal, invoice_count: i32, difference_total: Decimal, status: PosClosingStatus) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            pos_profile_id,
            opening_entry_id,
            closed_at,
            cashier_party_id,
            totals_by_method: None,
            grand_total,
            invoice_count,
            difference_total,
            status,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PosClosingEntryId {
        PosClosingEntryId(self.id)
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
    pub fn status(&self) -> &PosClosingStatus {
        &self.status
    }


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the totals_by_method field (chainable)
    pub fn with_totals_by_method(mut self, value: serde_json::Value) -> Self {
        self.totals_by_method = Some(value);
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
                "opening_entry_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.opening_entry_id = v; }
                }
                "closed_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.closed_at = v; }
                }
                "cashier_party_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.cashier_party_id = v; }
                }
                "totals_by_method" => {
                    if let Ok(v) = serde_json::from_value(value) { self.totals_by_method = v; }
                }
                "grand_total" => {
                    if let Ok(v) = serde_json::from_value(value) { self.grand_total = v; }
                }
                "invoice_count" => {
                    if let Ok(v) = serde_json::from_value(value) { self.invoice_count = v; }
                }
                "difference_total" => {
                    if let Ok(v) = serde_json::from_value(value) { self.difference_total = v; }
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

impl super::Entity for PosClosingEntry {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PosClosingEntry"
    }
}

impl backbone_core::PersistentEntity for PosClosingEntry {
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

impl backbone_orm::EntityRepoMeta for PosClosingEntry {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("pos_profile_id".to_string(), "uuid".to_string());
        m.insert("opening_entry_id".to_string(), "uuid".to_string());
        m.insert("cashier_party_id".to_string(), "uuid".to_string());
        m.insert("status".to_string(), "pos_closing_status".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for PosClosingEntry entity
///
/// Provides a fluent API for constructing PosClosingEntry instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PosClosingEntryBuilder {
    company_id: Option<Uuid>,
    pos_profile_id: Option<Uuid>,
    opening_entry_id: Option<Uuid>,
    closed_at: Option<DateTime<Utc>>,
    cashier_party_id: Option<Uuid>,
    totals_by_method: Option<serde_json::Value>,
    grand_total: Option<Decimal>,
    invoice_count: Option<i32>,
    difference_total: Option<Decimal>,
    status: Option<PosClosingStatus>,
}

impl PosClosingEntryBuilder {
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

    /// Set the opening_entry_id field (required)
    pub fn opening_entry_id(mut self, value: Uuid) -> Self {
        self.opening_entry_id = Some(value);
        self
    }

    /// Set the closed_at field (required)
    pub fn closed_at(mut self, value: DateTime<Utc>) -> Self {
        self.closed_at = Some(value);
        self
    }

    /// Set the cashier_party_id field (required)
    pub fn cashier_party_id(mut self, value: Uuid) -> Self {
        self.cashier_party_id = Some(value);
        self
    }

    /// Set the totals_by_method field (optional)
    pub fn totals_by_method(mut self, value: serde_json::Value) -> Self {
        self.totals_by_method = Some(value);
        self
    }

    /// Set the grand_total field (default: `Decimal::from(0)`)
    pub fn grand_total(mut self, value: Decimal) -> Self {
        self.grand_total = Some(value);
        self
    }

    /// Set the invoice_count field (default: `0`)
    pub fn invoice_count(mut self, value: i32) -> Self {
        self.invoice_count = Some(value);
        self
    }

    /// Set the difference_total field (default: `Decimal::from(0)`)
    pub fn difference_total(mut self, value: Decimal) -> Self {
        self.difference_total = Some(value);
        self
    }

    /// Set the status field (default: `PosClosingStatus::default()`)
    pub fn status(mut self, value: PosClosingStatus) -> Self {
        self.status = Some(value);
        self
    }

    /// Build the PosClosingEntry entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PosClosingEntry, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let pos_profile_id = self.pos_profile_id.ok_or_else(|| "pos_profile_id is required".to_string())?;
        let opening_entry_id = self.opening_entry_id.ok_or_else(|| "opening_entry_id is required".to_string())?;
        let closed_at = self.closed_at.ok_or_else(|| "closed_at is required".to_string())?;
        let cashier_party_id = self.cashier_party_id.ok_or_else(|| "cashier_party_id is required".to_string())?;

        Ok(PosClosingEntry {
            id: Uuid::new_v4(),
            company_id,
            pos_profile_id,
            opening_entry_id,
            closed_at,
            cashier_party_id,
            totals_by_method: self.totals_by_method,
            grand_total: self.grand_total.unwrap_or(Decimal::from(0)),
            invoice_count: self.invoice_count.unwrap_or(0),
            difference_total: self.difference_total.unwrap_or(Decimal::from(0)),
            status: self.status.unwrap_or(PosClosingStatus::default()),
            metadata: AuditMetadata::default(),
        })
    }
}
