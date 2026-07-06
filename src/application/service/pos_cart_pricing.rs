//! Inbound cart-pricing port (hand-authored, user-owned) — POS's side of the promo cart seam.
//!
//! POS rings a ticket from `unit_price`/`discount_amount` it is given. This is the seam that lets the
//! counter ask promo to price the WHOLE basket at once, so order-total discounts and bundles (which
//! span lines) land as per-line nets POS can ring. POS holds only the `CartPricingPort` trait + its
//! own DTOs; a composing service wires promo's `resolve_cart` behind it. **Zero normal Cargo edge** to
//! promo — the DTOs are the wire contract, duplicated per consumer by design (same posture as the
//! billing/payment ports POS already owns).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One ticket line to price, with the dimensions promo matches rules/bundles on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CartPriceLine {
    pub line_ref: Uuid,
    pub item_id: Uuid,
    pub item_group_id: Option<Uuid>,
    pub brand_id: Option<Uuid>,
    pub list_price: Decimal,
    pub quantity: Decimal,
}

/// The whole basket to price. Customer, group and coupon are ticket-wide.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CartPriceRequest {
    pub company_id: Uuid,
    pub customer_id: Option<Uuid>,
    pub customer_group_id: Option<Uuid>,
    pub coupon_code: Option<String>,
    pub lines: Vec<CartPriceLine>,
}

/// One line's resolved price: per-line unit price after line rules + the net it should carry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PricedCartLine {
    pub line_ref: Uuid,
    pub unit_price: Decimal,
    pub net_line_total: Decimal,
}

/// A free item a buy-X-get-Y bundle grants — rung on the ticket as a zero-priced line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PricedRewardLine {
    pub item_id: Uuid,
    pub quantity: Decimal,
}

/// The priced basket. `total` == Σ `net_line_total` (promo conserves this exactly). `reward_lines` are
/// extra free goods (zero-priced), not part of `total`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PricedCart {
    pub lines: Vec<PricedCartLine>,
    pub reward_lines: Vec<PricedRewardLine>,
    pub total: Decimal,
}

/// The composing service's rejection (promo unavailable, bad input).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CartPricingError {
    pub code: String,
    pub message: String,
}

/// The cart-pricing seam POS depends on. A composing service implements it over promo's `resolve_cart`.
#[async_trait::async_trait]
pub trait CartPricingPort: Send + Sync {
    async fn price_cart(&self, req: &CartPriceRequest) -> Result<PricedCart, CartPricingError>;
}
