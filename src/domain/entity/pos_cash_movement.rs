use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::PosCashMovementType;
use super::AuditMetadata;

/// Strongly-typed ID for PosCashMovement
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PosCashMovementId(pub Uuid);

impl PosCashMovementId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PosCashMovementId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PosCashMovementId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PosCashMovementId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PosCashMovementId> for Uuid {
    fn from(id: PosCashMovementId) -> Self { id.0 }
}

impl AsRef<Uuid> for PosCashMovementId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PosCashMovementId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PosCashMovement {
    pub id: Uuid,
    pub company_id: Uuid,
    pub pos_profile_id: Uuid,
    pub opening_entry_id: Uuid,
    pub cashier_party_id: Uuid,
    pub movement_type: PosCashMovementType,
    pub amount: Decimal,
    pub reason: Option<String>,
    pub moved_at: DateTime<Utc>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PosCashMovement {
    /// Create a builder for PosCashMovement
    pub fn builder() -> PosCashMovementBuilder {
        <PosCashMovementBuilder as Default>::default()
    }

    /// Create a new PosCashMovement with required fields
    pub fn new(company_id: Uuid, pos_profile_id: Uuid, opening_entry_id: Uuid, cashier_party_id: Uuid, movement_type: PosCashMovementType, amount: Decimal, moved_at: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            pos_profile_id,
            opening_entry_id,
            cashier_party_id,
            movement_type,
            amount,
            reason: None,
            moved_at,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PosCashMovementId {
        PosCashMovementId(self.id)
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

    /// Set the reason field (chainable)
    pub fn with_reason(mut self, value: String) -> Self {
        self.reason = Some(value);
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
                "cashier_party_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.cashier_party_id = v; }
                }
                "movement_type" => {
                    if let Ok(v) = serde_json::from_value(value) { self.movement_type = v; }
                }
                "amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.amount = v; }
                }
                "reason" => {
                    if let Ok(v) = serde_json::from_value(value) { self.reason = v; }
                }
                "moved_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.moved_at = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PosCashMovement {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PosCashMovement"
    }
}

impl backbone_core::PersistentEntity for PosCashMovement {
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

impl backbone_orm::EntityRepoMeta for PosCashMovement {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("pos_profile_id".to_string(), "uuid".to_string());
        m.insert("opening_entry_id".to_string(), "uuid".to_string());
        m.insert("cashier_party_id".to_string(), "uuid".to_string());
        m.insert("movement_type".to_string(), "pos_cash_movement_type".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for PosCashMovement entity
///
/// Provides a fluent API for constructing PosCashMovement instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PosCashMovementBuilder {
    company_id: Option<Uuid>,
    pos_profile_id: Option<Uuid>,
    opening_entry_id: Option<Uuid>,
    cashier_party_id: Option<Uuid>,
    movement_type: Option<PosCashMovementType>,
    amount: Option<Decimal>,
    reason: Option<String>,
    moved_at: Option<DateTime<Utc>>,
}

impl PosCashMovementBuilder {
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

    /// Set the cashier_party_id field (required)
    pub fn cashier_party_id(mut self, value: Uuid) -> Self {
        self.cashier_party_id = Some(value);
        self
    }

    /// Set the movement_type field (required)
    pub fn movement_type(mut self, value: PosCashMovementType) -> Self {
        self.movement_type = Some(value);
        self
    }

    /// Set the amount field (default: `Decimal::from(0)`)
    pub fn amount(mut self, value: Decimal) -> Self {
        self.amount = Some(value);
        self
    }

    /// Set the reason field (optional)
    pub fn reason(mut self, value: String) -> Self {
        self.reason = Some(value);
        self
    }

    /// Set the moved_at field (required)
    pub fn moved_at(mut self, value: DateTime<Utc>) -> Self {
        self.moved_at = Some(value);
        self
    }

    /// Build the PosCashMovement entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PosCashMovement, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let pos_profile_id = self.pos_profile_id.ok_or_else(|| "pos_profile_id is required".to_string())?;
        let opening_entry_id = self.opening_entry_id.ok_or_else(|| "opening_entry_id is required".to_string())?;
        let cashier_party_id = self.cashier_party_id.ok_or_else(|| "cashier_party_id is required".to_string())?;
        let movement_type = self.movement_type.ok_or_else(|| "movement_type is required".to_string())?;
        let moved_at = self.moved_at.ok_or_else(|| "moved_at is required".to_string())?;

        Ok(PosCashMovement {
            id: Uuid::new_v4(),
            company_id,
            pos_profile_id,
            opening_entry_id,
            cashier_party_id,
            movement_type,
            amount: self.amount.unwrap_or(Decimal::from(0)),
            reason: self.reason,
            moved_at,
            metadata: AuditMetadata::default(),
        })
    }
}
