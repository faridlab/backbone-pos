-- Down: drop pos.pos_cash_movements table
DROP TABLE IF EXISTS pos.pos_cash_movements CASCADE;
DROP FUNCTION IF EXISTS pos.pos_cash_movements_audit_timestamp() CASCADE;
