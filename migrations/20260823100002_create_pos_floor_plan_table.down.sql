-- Down: drop pos.pos_floor_plans table
DROP TABLE IF EXISTS pos.pos_floor_plans CASCADE;
DROP FUNCTION IF EXISTS pos.pos_floor_plans_audit_timestamp() CASCADE;
