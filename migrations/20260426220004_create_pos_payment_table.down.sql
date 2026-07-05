-- Down: drop pos.pos_payments table
DROP TABLE IF EXISTS pos.pos_payments CASCADE;
DROP FUNCTION IF EXISTS pos.pos_payments_audit_timestamp() CASCADE;
