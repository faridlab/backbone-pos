use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::PosInvoiceStatus;
use super::AuditMetadata;

/// Strongly-typed ID for PosInvoice
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PosInvoiceId(pub Uuid);

impl PosInvoiceId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PosInvoiceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PosInvoiceId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PosInvoiceId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PosInvoiceId> for Uuid {
    fn from(id: PosInvoiceId) -> Self { id.0 }
}

impl AsRef<Uuid> for PosInvoiceId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PosInvoiceId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PosInvoice {
    pub id: Uuid,
    pub company_id: Uuid,
    pub pos_profile_id: Uuid,
    pub opening_entry_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub receipt_number: String,
    pub posting_at: DateTime<Utc>,
    pub net_total: Decimal,
    pub tax_total: Decimal,
    pub grand_total: Decimal,
    pub rounding_adjustment: Decimal,
    pub rounded_total: Decimal,
    pub paid_total: Decimal,
    pub change_due: Decimal,
    pub billing_invoice_id: Option<Uuid>,
    pub is_return: bool,
    pub return_against: Option<Uuid>,
    pub status: PosInvoiceStatus,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PosInvoice {
    /// Create a builder for PosInvoice
    pub fn builder() -> PosInvoiceBuilder {
        PosInvoiceBuilder::default()
    }

    /// Create a new PosInvoice with required fields
    pub fn new(company_id: Uuid, pos_profile_id: Uuid, opening_entry_id: Uuid, receipt_number: String, posting_at: DateTime<Utc>, net_total: Decimal, tax_total: Decimal, grand_total: Decimal, rounding_adjustment: Decimal, rounded_total: Decimal, paid_total: Decimal, change_due: Decimal, is_return: bool, status: PosInvoiceStatus) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            pos_profile_id,
            opening_entry_id,
            branch_id: None,
            customer_id: None,
            receipt_number,
            posting_at,
            net_total,
            tax_total,
            grand_total,
            rounding_adjustment,
            rounded_total,
            paid_total,
            change_due,
            billing_invoice_id: None,
            is_return,
            return_against: None,
            status,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PosInvoiceId {
        PosInvoiceId(self.id)
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
    pub fn status(&self) -> &PosInvoiceStatus {
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

    /// Set the customer_id field (chainable)
    pub fn with_customer_id(mut self, value: Uuid) -> Self {
        self.customer_id = Some(value);
        self
    }

    /// Set the billing_invoice_id field (chainable)
    pub fn with_billing_invoice_id(mut self, value: Uuid) -> Self {
        self.billing_invoice_id = Some(value);
        self
    }

    /// Set the return_against field (chainable)
    pub fn with_return_against(mut self, value: Uuid) -> Self {
        self.return_against = Some(value);
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
                "branch_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.branch_id = v; }
                }
                "customer_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.customer_id = v; }
                }
                "receipt_number" => {
                    if let Ok(v) = serde_json::from_value(value) { self.receipt_number = v; }
                }
                "posting_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.posting_at = v; }
                }
                "net_total" => {
                    if let Ok(v) = serde_json::from_value(value) { self.net_total = v; }
                }
                "tax_total" => {
                    if let Ok(v) = serde_json::from_value(value) { self.tax_total = v; }
                }
                "grand_total" => {
                    if let Ok(v) = serde_json::from_value(value) { self.grand_total = v; }
                }
                "rounding_adjustment" => {
                    if let Ok(v) = serde_json::from_value(value) { self.rounding_adjustment = v; }
                }
                "rounded_total" => {
                    if let Ok(v) = serde_json::from_value(value) { self.rounded_total = v; }
                }
                "paid_total" => {
                    if let Ok(v) = serde_json::from_value(value) { self.paid_total = v; }
                }
                "change_due" => {
                    if let Ok(v) = serde_json::from_value(value) { self.change_due = v; }
                }
                "billing_invoice_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.billing_invoice_id = v; }
                }
                "is_return" => {
                    if let Ok(v) = serde_json::from_value(value) { self.is_return = v; }
                }
                "return_against" => {
                    if let Ok(v) = serde_json::from_value(value) { self.return_against = v; }
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

impl super::Entity for PosInvoice {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PosInvoice"
    }
}

impl backbone_core::PersistentEntity for PosInvoice {
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

impl backbone_orm::EntityRepoMeta for PosInvoice {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("pos_profile_id".to_string(), "uuid".to_string());
        m.insert("opening_entry_id".to_string(), "uuid".to_string());
        m.insert("branch_id".to_string(), "uuid".to_string());
        m.insert("customer_id".to_string(), "uuid".to_string());
        m.insert("billing_invoice_id".to_string(), "uuid".to_string());
        m.insert("status".to_string(), "pos_invoice_status".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["receipt_number"]
    }
}

/// Builder for PosInvoice entity
///
/// Provides a fluent API for constructing PosInvoice instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PosInvoiceBuilder {
    company_id: Option<Uuid>,
    pos_profile_id: Option<Uuid>,
    opening_entry_id: Option<Uuid>,
    branch_id: Option<Uuid>,
    customer_id: Option<Uuid>,
    receipt_number: Option<String>,
    posting_at: Option<DateTime<Utc>>,
    net_total: Option<Decimal>,
    tax_total: Option<Decimal>,
    grand_total: Option<Decimal>,
    rounding_adjustment: Option<Decimal>,
    rounded_total: Option<Decimal>,
    paid_total: Option<Decimal>,
    change_due: Option<Decimal>,
    billing_invoice_id: Option<Uuid>,
    is_return: Option<bool>,
    return_against: Option<Uuid>,
    status: Option<PosInvoiceStatus>,
}

impl PosInvoiceBuilder {
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

    /// Set the branch_id field (optional)
    pub fn branch_id(mut self, value: Uuid) -> Self {
        self.branch_id = Some(value);
        self
    }

    /// Set the customer_id field (optional)
    pub fn customer_id(mut self, value: Uuid) -> Self {
        self.customer_id = Some(value);
        self
    }

    /// Set the receipt_number field (required)
    pub fn receipt_number(mut self, value: String) -> Self {
        self.receipt_number = Some(value);
        self
    }

    /// Set the posting_at field (required)
    pub fn posting_at(mut self, value: DateTime<Utc>) -> Self {
        self.posting_at = Some(value);
        self
    }

    /// Set the net_total field (default: `Decimal::from(0)`)
    pub fn net_total(mut self, value: Decimal) -> Self {
        self.net_total = Some(value);
        self
    }

    /// Set the tax_total field (default: `Decimal::from(0)`)
    pub fn tax_total(mut self, value: Decimal) -> Self {
        self.tax_total = Some(value);
        self
    }

    /// Set the grand_total field (default: `Decimal::from(0)`)
    pub fn grand_total(mut self, value: Decimal) -> Self {
        self.grand_total = Some(value);
        self
    }

    /// Set the rounding_adjustment field (default: `Decimal::from(0)`)
    pub fn rounding_adjustment(mut self, value: Decimal) -> Self {
        self.rounding_adjustment = Some(value);
        self
    }

    /// Set the rounded_total field (default: `Decimal::from(0)`)
    pub fn rounded_total(mut self, value: Decimal) -> Self {
        self.rounded_total = Some(value);
        self
    }

    /// Set the paid_total field (default: `Decimal::from(0)`)
    pub fn paid_total(mut self, value: Decimal) -> Self {
        self.paid_total = Some(value);
        self
    }

    /// Set the change_due field (default: `Decimal::from(0)`)
    pub fn change_due(mut self, value: Decimal) -> Self {
        self.change_due = Some(value);
        self
    }

    /// Set the billing_invoice_id field (optional)
    pub fn billing_invoice_id(mut self, value: Uuid) -> Self {
        self.billing_invoice_id = Some(value);
        self
    }

    /// Set the is_return field (default: `false`)
    pub fn is_return(mut self, value: bool) -> Self {
        self.is_return = Some(value);
        self
    }

    /// Set the return_against field (optional)
    pub fn return_against(mut self, value: Uuid) -> Self {
        self.return_against = Some(value);
        self
    }

    /// Set the status field (default: `PosInvoiceStatus::default()`)
    pub fn status(mut self, value: PosInvoiceStatus) -> Self {
        self.status = Some(value);
        self
    }

    /// Build the PosInvoice entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PosInvoice, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let pos_profile_id = self.pos_profile_id.ok_or_else(|| "pos_profile_id is required".to_string())?;
        let opening_entry_id = self.opening_entry_id.ok_or_else(|| "opening_entry_id is required".to_string())?;
        let receipt_number = self.receipt_number.ok_or_else(|| "receipt_number is required".to_string())?;
        let posting_at = self.posting_at.ok_or_else(|| "posting_at is required".to_string())?;

        Ok(PosInvoice {
            id: Uuid::new_v4(),
            company_id,
            pos_profile_id,
            opening_entry_id,
            branch_id: self.branch_id,
            customer_id: self.customer_id,
            receipt_number,
            posting_at,
            net_total: self.net_total.unwrap_or(Decimal::from(0)),
            tax_total: self.tax_total.unwrap_or(Decimal::from(0)),
            grand_total: self.grand_total.unwrap_or(Decimal::from(0)),
            rounding_adjustment: self.rounding_adjustment.unwrap_or(Decimal::from(0)),
            rounded_total: self.rounded_total.unwrap_or(Decimal::from(0)),
            paid_total: self.paid_total.unwrap_or(Decimal::from(0)),
            change_due: self.change_due.unwrap_or(Decimal::from(0)),
            billing_invoice_id: self.billing_invoice_id,
            is_return: self.is_return.unwrap_or(false),
            return_against: self.return_against,
            status: self.status.unwrap_or(PosInvoiceStatus::default()),
            metadata: AuditMetadata::default(),
        })
    }
}
