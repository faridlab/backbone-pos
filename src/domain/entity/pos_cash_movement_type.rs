use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "pos_cash_movement_type", rename_all = "snake_case")]
pub enum PosCashMovementType {
    PayIn,
    PayOut,
    Drop,
    NoSale,
}

impl std::fmt::Display for PosCashMovementType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayIn => write!(f, "pay_in"),
            Self::PayOut => write!(f, "pay_out"),
            Self::Drop => write!(f, "drop"),
            Self::NoSale => write!(f, "no_sale"),
        }
    }
}

impl FromStr for PosCashMovementType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pay_in" => Ok(Self::PayIn),
            "pay_out" => Ok(Self::PayOut),
            "drop" => Ok(Self::Drop),
            "no_sale" => Ok(Self::NoSale),
            _ => Err(format!("Unknown PosCashMovementType variant: {}", s)),
        }
    }
}

impl Default for PosCashMovementType {
    fn default() -> Self {
        Self::PayIn
    }
}
