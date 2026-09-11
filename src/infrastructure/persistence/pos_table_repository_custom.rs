//! Hand-written PosTable reads (user-owned, never regenerated) — the custom methods on
//! [`PosTableRepository`]. Sibling to the generated newtype file, per the module's custom-code
//! convention. Holds the SQL per the 4-layer rule: services orchestrate, repositories hold SQL.
//!
//! The dining table is a UI-affordance record (geometry, seats); the write path cares about its
//! IDENTITY only — that a ticket seating itself at a table names a table that exists in the
//! caller's scope. Geometry is deliberately not read here.

use sqlx::Row;
use uuid::Uuid;

use crate::infrastructure::persistence::PosTableRepository;

/// A dining table as the ticket write path validates it: identity + the floor it belongs to (so a
/// caller can scope a register's seating by floor if it chooses).
pub struct TableRow {
    pub id: Uuid,
    pub pos_floor_plan_id: Uuid,
    pub name: Option<String>,
}

impl PosTableRepository {
    /// Read one dining table by id, on the CALLER'S scope-relayed transaction (the seating guard
    /// runs inside the ring/sync unit of work). `Ok(None)` = no such table visible to the caller's
    /// scope — the fence is the whole guard (ADR-0029).
    pub async fn fetch_table(
        &self,
        conn: &mut sqlx::PgConnection,
        pos_table_id: Uuid,
    ) -> Result<Option<TableRow>, sqlx::Error> {
        let row = sqlx::query(
            r#"SELECT id, pos_floor_plan_id, name
               FROM pos.pos_tables
               WHERE id=$1 AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(pos_table_id)
        .fetch_optional(conn)
        .await?;
        Ok(row.map(|r| TableRow {
            id: r.get("id"),
            pos_floor_plan_id: r.get("pos_floor_plan_id"),
            name: r.get("name"),
        }))
    }
}
