//! Hand-written PosTable reads (user-owned, never regenerated) — the custom methods on
//! [`PosTableRepository`]. Sibling to the generated newtype file, per the module's custom-code
//! convention. Holds the SQL per the 4-layer rule: services orchestrate, repositories hold SQL.
//!
//! The dining table is a UI-affordance record (geometry, seats); the write path cares about its
//! IDENTITY only — that a ticket seating itself at a table names a table that exists in the
//! caller's tenant. Geometry is deliberately not read here.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use backbone_orm::company_scope;

use crate::infrastructure::persistence::PosTableRepository;

/// A dining table as the ticket write path validates it: identity + the floor it belongs to (so a
/// caller can scope a register's seating by floor if it chooses).
pub struct TableRow {
    pub id: Uuid,
    pub pos_floor_plan_id: Uuid,
    pub name: Option<String>,
}

impl PosTableRepository {
    /// Read one dining table by id. `Ok(None)` = no such table in this tenant (the explicit
    /// `company_id = $2` filter is defense-in-depth ON TOP of the RLS fence; the caller wraps this
    /// in `with_company_scope(Some(company))`).
    pub async fn fetch_table(
        &self,
        pool: &PgPool,
        pos_table_id: Uuid,
        company_id: Uuid,
    ) -> Result<Option<TableRow>, sqlx::Error> {
        let row = company_scope::fetch_optional_row_scoped(
            pool,
            sqlx::query(
                r#"SELECT id, pos_floor_plan_id, name
                   FROM pos.pos_tables
                   WHERE id=$1 AND company_id=$2 AND (metadata->>'deleted_at') IS NULL"#,
            )
            .bind(pos_table_id)
            .bind(company_id),
        )
        .await?;
        Ok(row.map(|r| TableRow {
            id: r.get("id"),
            pos_floor_plan_id: r.get("pos_floor_plan_id"),
            name: r.get("name"),
        }))
    }
}
