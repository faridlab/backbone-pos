//! Hand-written PosDiscount reads (user-owned, never regenerated) — the custom methods on
//! [`PosDiscountRepository`]. Sibling to the generated newtype file, per the module's custom-code
//! convention. Holds the SQL per the 4-layer rule: services orchestrate, repositories hold SQL.
//!
//! The order-level discount master is the server-side source of the discount RATE: a ring names a
//! discount by id, and the percentage applied is ALWAYS the one stored on this company's master row
//! — never a rate echoed back by the client (the offline-sync trust posture: client identity, server
//! money).

use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use backbone_orm::company_scope;

use crate::infrastructure::persistence::PosDiscountRepository;

/// A discount master row as the ticket compute reads it: the percentage is a FRACTION (0.1 = 10%),
/// stored `decimal(6,4)` exactly like the register's retired flat tax rate was.
pub struct DiscountRow {
    pub id: Uuid,
    pub name: String,
    pub percentage: Decimal,
}

impl PosDiscountRepository {
    /// Read one discount master row by id. `Ok(None)` = no such discount in this tenant (soft-deleted
    /// masters are retired — a ring naming one is refused, not silently discounted). The explicit
    /// `company_id = $2` filter is defense-in-depth ON TOP of the RLS fence; the caller wraps this in
    /// `with_company_scope(Some(company))`.
    pub async fn fetch_discount(
        &self,
        pool: &PgPool,
        discount_id: Uuid,
        company_id: Uuid,
    ) -> Result<Option<DiscountRow>, sqlx::Error> {
        let row = company_scope::fetch_optional_row_scoped(
            pool,
            sqlx::query(
                r#"SELECT id, name, percentage
                   FROM pos.pos_discounts
                   WHERE id=$1 AND company_id=$2 AND (metadata->>'deleted_at') IS NULL"#,
            )
            .bind(discount_id)
            .bind(company_id),
        )
        .await?;
        Ok(row.map(|r| DiscountRow {
            id: r.get("id"),
            name: r.get("name"),
            percentage: r.get("percentage"),
        }))
    }
}
