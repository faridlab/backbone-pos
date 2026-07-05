use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "pos_invoice_status", rename_all = "snake_case")]
pub enum PosInvoiceStatus {
    Draft,
    Paid,
    Consolidated,
    Returned,
    Void,
}

impl std::fmt::Display for PosInvoiceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Draft => write!(f, "draft"),
            Self::Paid => write!(f, "paid"),
            Self::Consolidated => write!(f, "consolidated"),
            Self::Returned => write!(f, "returned"),
            Self::Void => write!(f, "void"),
        }
    }
}

impl FromStr for PosInvoiceStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "draft" => Ok(Self::Draft),
            "paid" => Ok(Self::Paid),
            "consolidated" => Ok(Self::Consolidated),
            "returned" => Ok(Self::Returned),
            "void" => Ok(Self::Void),
            _ => Err(format!("Unknown PosInvoiceStatus variant: {}", s)),
        }
    }
}

impl Default for PosInvoiceStatus {
    fn default() -> Self {
        Self::Draft
    }
}
