-- Down: drop pos.pos_discounts table
DROP TABLE IF EXISTS pos.pos_discounts CASCADE;
DROP FUNCTION IF EXISTS pos.pos_discounts_audit_timestamp() CASCADE;
