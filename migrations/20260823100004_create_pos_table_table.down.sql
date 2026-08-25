-- Down: drop pos.pos_tables table
DROP TABLE IF EXISTS pos.pos_tables CASCADE;
DROP FUNCTION IF EXISTS pos.pos_tables_audit_timestamp() CASCADE;
