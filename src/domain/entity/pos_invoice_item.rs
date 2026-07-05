use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;
use super::AuditMetadata;

/// Strongly-typed ID for PosInvoiceItem
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PosInvoiceItemId(pub Uuid);

impl PosInvoiceItemId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PosInvoiceItemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PosInvoiceItemId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PosInvoiceItemId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PosInvoiceItemId> for Uuid {
    fn from(id: PosInvoiceItemId) -> Self { id.0 }
}

impl AsRef<Uuid> for PosInvoiceItemId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PosInvoiceItemId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PosInvoiceItem {
    pub id: Uuid,
    pub pos_invoice_id: Uuid,
    pub item_id: Uuid,
    pub description: Option<String>,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    pub discount_amount: Decimal,
    pub net_amount: Decimal,
    pub revenue_account_id: Option<Uuid>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PosInvoiceItem {
    /// Create a builder for PosInvoiceItem
    pub fn builder() -> PosInvoiceItemBuilder {
        PosInvoiceItemBuilder::default()
    }

    /// Create a new PosInvoiceItem with required fields
    pub fn new(pos_invoice_id: Uuid, item_id: Uuid, quantity: Decimal, unit_price: Decimal, discount_amount: Decimal, net_amount: Decimal) -> Self {
        Self {
            id: Uuid::new_v4(),
            pos_invoice_id,
            item_id,
            description: None,
            quantity,
            unit_price,
            discount_amount,
            net_amount,
            revenue_account_id: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PosInvoiceItemId {
        PosInvoiceItemId(self.id)
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

    /// Set the description field (chainable)
    pub fn with_description(mut self, value: String) -> Self {
        self.description = Some(value);
        self
    }

    /// Set the revenue_account_id field (chainable)
    pub fn with_revenue_account_id(mut self, value: Uuid) -> Self {
        self.revenue_account_id = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "pos_invoice_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.pos_invoice_id = v; }
                }
                "item_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.item_id = v; }
                }
                "description" => {
                    if let Ok(v) = serde_json::from_value(value) { self.description = v; }
                }
                "quantity" => {
                    if let Ok(v) = serde_json::from_value(value) { self.quantity = v; }
                }
                "unit_price" => {
                    if let Ok(v) = serde_json::from_value(value) { self.unit_price = v; }
                }
                "discount_amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.discount_amount = v; }
                }
                "net_amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.net_amount = v; }
                }
                "revenue_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.revenue_account_id = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PosInvoiceItem {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PosInvoiceItem"
    }
}

impl backbone_core::PersistentEntity for PosInvoiceItem {
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

impl backbone_orm::EntityRepoMeta for PosInvoiceItem {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("pos_invoice_id".to_string(), "uuid".to_string());
        m.insert("item_id".to_string(), "uuid".to_string());
        m.insert("revenue_account_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn relations() -> &'static [(&'static str, &'static str, &'static str)] {
        &[("invoice", "pos_invoices", "posInvoiceId")]
    }
}

/// Builder for PosInvoiceItem entity
///
/// Provides a fluent API for constructing PosInvoiceItem instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PosInvoiceItemBuilder {
    pos_invoice_id: Option<Uuid>,
    item_id: Option<Uuid>,
    description: Option<String>,
    quantity: Option<Decimal>,
    unit_price: Option<Decimal>,
    discount_amount: Option<Decimal>,
    net_amount: Option<Decimal>,
    revenue_account_id: Option<Uuid>,
}

impl PosInvoiceItemBuilder {
    /// Set the pos_invoice_id field (required)
    pub fn pos_invoice_id(mut self, value: Uuid) -> Self {
        self.pos_invoice_id = Some(value);
        self
    }

    /// Set the item_id field (required)
    pub fn item_id(mut self, value: Uuid) -> Self {
        self.item_id = Some(value);
        self
    }

    /// Set the description field (optional)
    pub fn description(mut self, value: String) -> Self {
        self.description = Some(value);
        self
    }

    /// Set the quantity field (required)
    pub fn quantity(mut self, value: Decimal) -> Self {
        self.quantity = Some(value);
        self
    }

    /// Set the unit_price field (required)
    pub fn unit_price(mut self, value: Decimal) -> Self {
        self.unit_price = Some(value);
        self
    }

    /// Set the discount_amount field (default: `Decimal::from(0)`)
    pub fn discount_amount(mut self, value: Decimal) -> Self {
        self.discount_amount = Some(value);
        self
    }

    /// Set the net_amount field (default: `Decimal::from(0)`)
    pub fn net_amount(mut self, value: Decimal) -> Self {
        self.net_amount = Some(value);
        self
    }

    /// Set the revenue_account_id field (optional)
    pub fn revenue_account_id(mut self, value: Uuid) -> Self {
        self.revenue_account_id = Some(value);
        self
    }

    /// Build the PosInvoiceItem entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PosInvoiceItem, String> {
        let pos_invoice_id = self.pos_invoice_id.ok_or_else(|| "pos_invoice_id is required".to_string())?;
        let item_id = self.item_id.ok_or_else(|| "item_id is required".to_string())?;
        let quantity = self.quantity.ok_or_else(|| "quantity is required".to_string())?;
        let unit_price = self.unit_price.ok_or_else(|| "unit_price is required".to_string())?;

        Ok(PosInvoiceItem {
            id: Uuid::new_v4(),
            pos_invoice_id,
            item_id,
            description: self.description,
            quantity,
            unit_price,
            discount_amount: self.discount_amount.unwrap_or(Decimal::from(0)),
            net_amount: self.net_amount.unwrap_or(Decimal::from(0)),
            revenue_account_id: self.revenue_account_id,
            metadata: AuditMetadata::default(),
        })
    }
}
