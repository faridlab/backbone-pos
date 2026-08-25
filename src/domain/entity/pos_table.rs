use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use super::AuditMetadata;

/// Strongly-typed ID for PosTable
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PosTableId(pub Uuid);

impl PosTableId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PosTableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PosTableId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PosTableId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PosTableId> for Uuid {
    fn from(id: PosTableId) -> Self { id.0 }
}

impl AsRef<Uuid> for PosTableId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PosTableId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PosTable {
    pub id: Uuid,
    pub company_id: Uuid,
    pub pos_floor_plan_id: Uuid,
    pub name: Option<String>,
    pub seats: Option<i32>,
    pub shape: Option<String>,
    pub position: Option<serde_json::Value>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PosTable {
    /// Create a builder for PosTable
    pub fn builder() -> PosTableBuilder {
        <PosTableBuilder as Default>::default()
    }

    /// Create a new PosTable with required fields
    pub fn new(company_id: Uuid, pos_floor_plan_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            pos_floor_plan_id,
            name: None,
            seats: None,
            shape: None,
            position: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PosTableId {
        PosTableId(self.id)
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

    /// Set the name field (chainable)
    pub fn with_name(mut self, value: String) -> Self {
        self.name = Some(value);
        self
    }

    /// Set the seats field (chainable)
    pub fn with_seats(mut self, value: i32) -> Self {
        self.seats = Some(value);
        self
    }

    /// Set the shape field (chainable)
    pub fn with_shape(mut self, value: String) -> Self {
        self.shape = Some(value);
        self
    }

    /// Set the position field (chainable)
    pub fn with_position(mut self, value: serde_json::Value) -> Self {
        self.position = Some(value);
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
                "pos_floor_plan_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.pos_floor_plan_id = v; }
                }
                "name" => {
                    if let Ok(v) = serde_json::from_value(value) { self.name = v; }
                }
                "seats" => {
                    if let Ok(v) = serde_json::from_value(value) { self.seats = v; }
                }
                "shape" => {
                    if let Ok(v) = serde_json::from_value(value) { self.shape = v; }
                }
                "position" => {
                    if let Ok(v) = serde_json::from_value(value) { self.position = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PosTable {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PosTable"
    }
}

impl backbone_core::PersistentEntity for PosTable {
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

impl backbone_orm::EntityRepoMeta for PosTable {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("pos_floor_plan_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
    fn relations() -> &'static [(&'static str, &'static str, &'static str)] {
        &[("floorPlan", "pos_floor_plans", "posFloorPlanId")]
    }
}

/// Builder for PosTable entity
///
/// Provides a fluent API for constructing PosTable instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PosTableBuilder {
    company_id: Option<Uuid>,
    pos_floor_plan_id: Option<Uuid>,
    name: Option<String>,
    seats: Option<i32>,
    shape: Option<String>,
    position: Option<serde_json::Value>,
}

impl PosTableBuilder {
    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Set the pos_floor_plan_id field (required)
    pub fn pos_floor_plan_id(mut self, value: Uuid) -> Self {
        self.pos_floor_plan_id = Some(value);
        self
    }

    /// Set the name field (optional)
    pub fn name(mut self, value: String) -> Self {
        self.name = Some(value);
        self
    }

    /// Set the seats field (optional)
    pub fn seats(mut self, value: i32) -> Self {
        self.seats = Some(value);
        self
    }

    /// Set the shape field (optional)
    pub fn shape(mut self, value: String) -> Self {
        self.shape = Some(value);
        self
    }

    /// Set the position field (optional)
    pub fn position(mut self, value: serde_json::Value) -> Self {
        self.position = Some(value);
        self
    }

    /// Build the PosTable entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PosTable, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let pos_floor_plan_id = self.pos_floor_plan_id.ok_or_else(|| "pos_floor_plan_id is required".to_string())?;

        Ok(PosTable {
            id: Uuid::new_v4(),
            company_id,
            pos_floor_plan_id,
            name: self.name,
            seats: self.seats,
            shape: self.shape,
            position: self.position,
            metadata: AuditMetadata::default(),
        })
    }
}
