use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "pos_payment_method", rename_all = "snake_case")]
pub enum PosPaymentMethod {
    Cash,
    Card,
    Qris,
    EWallet,
    BankTransfer,
    VirtualAccount,
}

impl std::fmt::Display for PosPaymentMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cash => write!(f, "cash"),
            Self::Card => write!(f, "card"),
            Self::Qris => write!(f, "qris"),
            Self::EWallet => write!(f, "e_wallet"),
            Self::BankTransfer => write!(f, "bank_transfer"),
            Self::VirtualAccount => write!(f, "virtual_account"),
        }
    }
}

impl FromStr for PosPaymentMethod {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "cash" => Ok(Self::Cash),
            "card" => Ok(Self::Card),
            "qris" => Ok(Self::Qris),
            "e_wallet" => Ok(Self::EWallet),
            "bank_transfer" => Ok(Self::BankTransfer),
            "virtual_account" => Ok(Self::VirtualAccount),
            _ => Err(format!("Unknown PosPaymentMethod variant: {}", s)),
        }
    }
}

impl Default for PosPaymentMethod {
    fn default() -> Self {
        Self::Cash
    }
}
