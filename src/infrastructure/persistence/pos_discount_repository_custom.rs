//! Hand-written PosDiscount reads (user-owned, never regenerated) — the custom methods on
//! [`PosDiscountRepository`]. Sibling to the generated newtype file, per the module's custom-code
//! convention. Holds the SQL per the 4-layer rule: services orchestrate, repositories hold SQL.
//!
//! The order-level discount master is the server-side source of the discount RATE: a ring names a
//! discount by id, and the percentage applied is ALWAYS the one stored on the caller's scoped
//! master row — never a rate echoed back by the client (the offline-sync trust posture: client
//! identity, server money).

use rust_decimal::Decimal;
use sqlx::Row;
use uuid::Uuid;

use crate::infrastructure::persistence::PosDiscountRepository;

/// A discount master row as the ticket compute reads it: the percentage is a FRACTION (0.1 = 10%),
/// stored `decimal(6,4)` exactly like the register's retired flat tax rate was.
pub struct DiscountRow {
    pub id: Uuid,
    pub name: String,
    pub percentage: Decimal,
}

impl PosDiscountRepository {
    /// Read one discount master row by id, on the CALLER'S scope-relayed transaction (the discount
    /// gate runs inside the ring/sync unit of work). `Ok(None)` = no such discount visible to the
    /// caller's scope (soft-deleted masters are retired — a ring naming one is refused, not silently
    /// discounted). The fence is the whole guard (ADR-0029).
    pub async fn fetch_discount(
        &self,
        conn: &mut sqlx::PgConnection,
        discount_id: Uuid,
    ) -> Result<Option<DiscountRow>, sqlx::Error> {
        let row = sqlx::query(
            r#"SELECT id, name, percentage
               FROM pos.pos_discounts
               WHERE id=$1 AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(discount_id)
        .fetch_optional(conn)
        .await?;
        Ok(row.map(|r| DiscountRow {
            id: r.get("id"),
            name: r.get("name"),
            percentage: r.get("percentage"),
        }))
    }
}
