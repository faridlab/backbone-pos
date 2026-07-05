use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::PosPaymentMethod;
use super::AuditMetadata;

/// Strongly-typed ID for PosPayment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PosPaymentId(pub Uuid);

impl PosPaymentId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PosPaymentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PosPaymentId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PosPaymentId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PosPaymentId> for Uuid {
    fn from(id: PosPaymentId) -> Self { id.0 }
}

impl AsRef<Uuid> for PosPaymentId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PosPaymentId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PosPayment {
    pub id: Uuid,
    pub pos_invoice_id: Uuid,
    pub payment_method: PosPaymentMethod,
    pub amount: Decimal,
    pub reference_no: Option<String>,
    pub payment_entry_id: Option<Uuid>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PosPayment {
    /// Create a builder for PosPayment
    pub fn builder() -> PosPaymentBuilder {
        PosPaymentBuilder::default()
    }

    /// Create a new PosPayment with required fields
    pub fn new(pos_invoice_id: Uuid, payment_method: PosPaymentMethod, amount: Decimal) -> Self {
        Self {
            id: Uuid::new_v4(),
            pos_invoice_id,
            payment_method,
            amount,
            reference_no: None,
            payment_entry_id: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PosPaymentId {
        PosPaymentId(self.id)
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

    /// Set the reference_no field (chainable)
    pub fn with_reference_no(mut self, value: String) -> Self {
        self.reference_no = Some(value);
        self
    }

    /// Set the payment_entry_id field (chainable)
    pub fn with_payment_entry_id(mut self, value: Uuid) -> Self {
        self.payment_entry_id = Some(value);
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
                "payment_method" => {
                    if let Ok(v) = serde_json::from_value(value) { self.payment_method = v; }
                }
                "amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.amount = v; }
                }
                "reference_no" => {
                    if let Ok(v) = serde_json::from_value(value) { self.reference_no = v; }
                }
                "payment_entry_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.payment_entry_id = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PosPayment {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PosPayment"
    }
}

impl backbone_core::PersistentEntity for PosPayment {
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

impl backbone_orm::EntityRepoMeta for PosPayment {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("pos_invoice_id".to_string(), "uuid".to_string());
        m.insert("payment_entry_id".to_string(), "uuid".to_string());
        m.insert("payment_method".to_string(), "pos_payment_method".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn relations() -> &'static [(&'static str, &'static str, &'static str)] {
        &[("invoice", "pos_invoices", "posInvoiceId")]
    }
}

/// Builder for PosPayment entity
///
/// Provides a fluent API for constructing PosPayment instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PosPaymentBuilder {
    pos_invoice_id: Option<Uuid>,
    payment_method: Option<PosPaymentMethod>,
    amount: Option<Decimal>,
    reference_no: Option<String>,
    payment_entry_id: Option<Uuid>,
}

impl PosPaymentBuilder {
    /// Set the pos_invoice_id field (required)
    pub fn pos_invoice_id(mut self, value: Uuid) -> Self {
        self.pos_invoice_id = Some(value);
        self
    }

    /// Set the payment_method field (required)
    pub fn payment_method(mut self, value: PosPaymentMethod) -> Self {
        self.payment_method = Some(value);
        self
    }

    /// Set the amount field (required)
    pub fn amount(mut self, value: Decimal) -> Self {
        self.amount = Some(value);
        self
    }

    /// Set the reference_no field (optional)
    pub fn reference_no(mut self, value: String) -> Self {
        self.reference_no = Some(value);
        self
    }

    /// Set the payment_entry_id field (optional)
    pub fn payment_entry_id(mut self, value: Uuid) -> Self {
        self.payment_entry_id = Some(value);
        self
    }

    /// Build the PosPayment entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PosPayment, String> {
        let pos_invoice_id = self.pos_invoice_id.ok_or_else(|| "pos_invoice_id is required".to_string())?;
        let payment_method = self.payment_method.ok_or_else(|| "payment_method is required".to_string())?;
        let amount = self.amount.ok_or_else(|| "amount is required".to_string())?;

        Ok(PosPayment {
            id: Uuid::new_v4(),
            pos_invoice_id,
            payment_method,
            amount,
            reference_no: self.reference_no,
            payment_entry_id: self.payment_entry_id,
            metadata: AuditMetadata::default(),
        })
    }
}
