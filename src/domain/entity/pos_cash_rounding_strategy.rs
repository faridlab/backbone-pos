use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "pos_cash_rounding_strategy", rename_all = "snake_case")]
pub enum PosCashRoundingStrategy {
    None,
    HalfUp,
}

impl std::fmt::Display for PosCashRoundingStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::HalfUp => write!(f, "half_up"),
        }
    }
}

impl FromStr for PosCashRoundingStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "half_up" => Ok(Self::HalfUp),
            _ => Err(format!("Unknown PosCashRoundingStrategy variant: {}", s)),
        }
    }
}

impl Default for PosCashRoundingStrategy {
    fn default() -> Self {
        Self::None
    }
}
