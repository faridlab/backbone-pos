use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "pos_closing_status", rename_all = "snake_case")]
pub enum PosClosingStatus {
    Draft,
    Submitted,
    Reconciled,
}

impl std::fmt::Display for PosClosingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Draft => write!(f, "draft"),
            Self::Submitted => write!(f, "submitted"),
            Self::Reconciled => write!(f, "reconciled"),
        }
    }
}

impl FromStr for PosClosingStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "draft" => Ok(Self::Draft),
            "submitted" => Ok(Self::Submitted),
            "reconciled" => Ok(Self::Reconciled),
            _ => Err(format!("Unknown PosClosingStatus variant: {}", s)),
        }
    }
}

impl Default for PosClosingStatus {
    fn default() -> Self {
        Self::Draft
    }
}
