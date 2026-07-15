use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;
use super::AuditMetadata;

/// Strongly-typed ID for PosProfile
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PosProfileId(pub Uuid);

impl PosProfileId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PosProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PosProfileId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PosProfileId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PosProfileId> for Uuid {
    fn from(id: PosProfileId) -> Self { id.0 }
}

impl AsRef<Uuid> for PosProfileId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PosProfileId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PosProfile {
    pub id: Uuid,
    pub company_id: Uuid,
    pub branch_id: Option<Uuid>,
    pub name: String,
    pub default_customer_id: Option<Uuid>,
    pub currency: String,
    pub income_account_id: Option<Uuid>,
    pub receivable_account_id: Option<Uuid>,
    pub cash_account_id: Option<Uuid>,
    pub write_off_account_id: Option<Uuid>,
    pub tax_account_id: Option<Uuid>,
    pub tax_rate: Decimal,
    pub warehouse_id: Option<Uuid>,
    pub cogs_account_id: Option<Uuid>,
    pub inventory_account_id: Option<Uuid>,
    pub allow_discount: bool,
    pub is_active: bool,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PosProfile {
    /// Create a builder for PosProfile
    pub fn builder() -> PosProfileBuilder {
        PosProfileBuilder::default()
    }

    /// Create a new PosProfile with required fields
    pub fn new(company_id: Uuid, name: String, currency: String, tax_rate: Decimal, allow_discount: bool, is_active: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            branch_id: None,
            name,
            default_customer_id: None,
            currency,
            income_account_id: None,
            receivable_account_id: None,
            cash_account_id: None,
            write_off_account_id: None,
            tax_account_id: None,
            tax_rate,
            warehouse_id: None,
            cogs_account_id: None,
            inventory_account_id: None,
            allow_discount,
            is_active,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PosProfileId {
        PosProfileId(self.id)
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

    /// Set the branch_id field (chainable)
    pub fn with_branch_id(mut self, value: Uuid) -> Self {
        self.branch_id = Some(value);
        self
    }

    /// Set the default_customer_id field (chainable)
    pub fn with_default_customer_id(mut self, value: Uuid) -> Self {
        self.default_customer_id = Some(value);
        self
    }

    /// Set the income_account_id field (chainable)
    pub fn with_income_account_id(mut self, value: Uuid) -> Self {
        self.income_account_id = Some(value);
        self
    }

    /// Set the receivable_account_id field (chainable)
    pub fn with_receivable_account_id(mut self, value: Uuid) -> Self {
        self.receivable_account_id = Some(value);
        self
    }

    /// Set the cash_account_id field (chainable)
    pub fn with_cash_account_id(mut self, value: Uuid) -> Self {
        self.cash_account_id = Some(value);
        self
    }

    /// Set the write_off_account_id field (chainable)
    pub fn with_write_off_account_id(mut self, value: Uuid) -> Self {
        self.write_off_account_id = Some(value);
        self
    }

    /// Set the tax_account_id field (chainable)
    pub fn with_tax_account_id(mut self, value: Uuid) -> Self {
        self.tax_account_id = Some(value);
        self
    }

    /// Set the warehouse_id field (chainable)
    pub fn with_warehouse_id(mut self, value: Uuid) -> Self {
        self.warehouse_id = Some(value);
        self
    }

    /// Set the cogs_account_id field (chainable)
    pub fn with_cogs_account_id(mut self, value: Uuid) -> Self {
        self.cogs_account_id = Some(value);
        self
    }

    /// Set the inventory_account_id field (chainable)
    pub fn with_inventory_account_id(mut self, value: Uuid) -> Self {
        self.inventory_account_id = Some(value);
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
                "branch_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.branch_id = v; }
                }
                "name" => {
                    if let Ok(v) = serde_json::from_value(value) { self.name = v; }
                }
                "default_customer_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.default_customer_id = v; }
                }
                "currency" => {
                    if let Ok(v) = serde_json::from_value(value) { self.currency = v; }
                }
                "income_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.income_account_id = v; }
                }
                "receivable_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.receivable_account_id = v; }
                }
                "cash_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.cash_account_id = v; }
                }
                "write_off_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.write_off_account_id = v; }
                }
                "tax_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.tax_account_id = v; }
                }
                "tax_rate" => {
                    if let Ok(v) = serde_json::from_value(value) { self.tax_rate = v; }
                }
                "warehouse_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.warehouse_id = v; }
                }
                "cogs_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.cogs_account_id = v; }
                }
                "inventory_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.inventory_account_id = v; }
                }
                "allow_discount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.allow_discount = v; }
                }
                "is_active" => {
                    if let Ok(v) = serde_json::from_value(value) { self.is_active = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PosProfile {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PosProfile"
    }
}

impl backbone_core::PersistentEntity for PosProfile {
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

impl backbone_orm::EntityRepoMeta for PosProfile {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("branch_id".to_string(), "uuid".to_string());
        m.insert("default_customer_id".to_string(), "uuid".to_string());
        m.insert("income_account_id".to_string(), "uuid".to_string());
        m.insert("receivable_account_id".to_string(), "uuid".to_string());
        m.insert("cash_account_id".to_string(), "uuid".to_string());
        m.insert("write_off_account_id".to_string(), "uuid".to_string());
        m.insert("tax_account_id".to_string(), "uuid".to_string());
        m.insert("warehouse_id".to_string(), "uuid".to_string());
        m.insert("cogs_account_id".to_string(), "uuid".to_string());
        m.insert("inventory_account_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["name", "currency"]
    }
}

/// Builder for PosProfile entity
///
/// Provides a fluent API for constructing PosProfile instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PosProfileBuilder {
    company_id: Option<Uuid>,
    branch_id: Option<Uuid>,
    name: Option<String>,
    default_customer_id: Option<Uuid>,
    currency: Option<String>,
    income_account_id: Option<Uuid>,
    receivable_account_id: Option<Uuid>,
    cash_account_id: Option<Uuid>,
    write_off_account_id: Option<Uuid>,
    tax_account_id: Option<Uuid>,
    tax_rate: Option<Decimal>,
    warehouse_id: Option<Uuid>,
    cogs_account_id: Option<Uuid>,
    inventory_account_id: Option<Uuid>,
    allow_discount: Option<bool>,
    is_active: Option<bool>,
}

impl PosProfileBuilder {
    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Set the branch_id field (optional)
    pub fn branch_id(mut self, value: Uuid) -> Self {
        self.branch_id = Some(value);
        self
    }

    /// Set the name field (required)
    pub fn name(mut self, value: String) -> Self {
        self.name = Some(value);
        self
    }

    /// Set the default_customer_id field (optional)
    pub fn default_customer_id(mut self, value: Uuid) -> Self {
        self.default_customer_id = Some(value);
        self
    }

    /// Set the currency field (default: `"IDR".to_string()`)
    pub fn currency(mut self, value: String) -> Self {
        self.currency = Some(value);
        self
    }

    /// Set the income_account_id field (optional)
    pub fn income_account_id(mut self, value: Uuid) -> Self {
        self.income_account_id = Some(value);
        self
    }

    /// Set the receivable_account_id field (optional)
    pub fn receivable_account_id(mut self, value: Uuid) -> Self {
        self.receivable_account_id = Some(value);
        self
    }

    /// Set the cash_account_id field (optional)
    pub fn cash_account_id(mut self, value: Uuid) -> Self {
        self.cash_account_id = Some(value);
        self
    }

    /// Set the write_off_account_id field (optional)
    pub fn write_off_account_id(mut self, value: Uuid) -> Self {
        self.write_off_account_id = Some(value);
        self
    }

    /// Set the tax_account_id field (optional)
    pub fn tax_account_id(mut self, value: Uuid) -> Self {
        self.tax_account_id = Some(value);
        self
    }

    /// Set the tax_rate field (default: `Decimal::from(0)`)
    pub fn tax_rate(mut self, value: Decimal) -> Self {
        self.tax_rate = Some(value);
        self
    }

    /// Set the warehouse_id field (optional)
    pub fn warehouse_id(mut self, value: Uuid) -> Self {
        self.warehouse_id = Some(value);
        self
    }

    /// Set the cogs_account_id field (optional)
    pub fn cogs_account_id(mut self, value: Uuid) -> Self {
        self.cogs_account_id = Some(value);
        self
    }

    /// Set the inventory_account_id field (optional)
    pub fn inventory_account_id(mut self, value: Uuid) -> Self {
        self.inventory_account_id = Some(value);
        self
    }

    /// Set the allow_discount field (default: `true`)
    pub fn allow_discount(mut self, value: bool) -> Self {
        self.allow_discount = Some(value);
        self
    }

    /// Set the is_active field (default: `true`)
    pub fn is_active(mut self, value: bool) -> Self {
        self.is_active = Some(value);
        self
    }

    /// Build the PosProfile entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PosProfile, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let name = self.name.ok_or_else(|| "name is required".to_string())?;

        Ok(PosProfile {
            id: Uuid::new_v4(),
            company_id,
            branch_id: self.branch_id,
            name,
            default_customer_id: self.default_customer_id,
            currency: self.currency.unwrap_or("IDR".to_string()),
            income_account_id: self.income_account_id,
            receivable_account_id: self.receivable_account_id,
            cash_account_id: self.cash_account_id,
            write_off_account_id: self.write_off_account_id,
            tax_account_id: self.tax_account_id,
            tax_rate: self.tax_rate.unwrap_or(Decimal::from(0)),
            warehouse_id: self.warehouse_id,
            cogs_account_id: self.cogs_account_id,
            inventory_account_id: self.inventory_account_id,
            allow_discount: self.allow_discount.unwrap_or(true),
            is_active: self.is_active.unwrap_or(true),
            metadata: AuditMetadata::default(),
        })
    }
}
