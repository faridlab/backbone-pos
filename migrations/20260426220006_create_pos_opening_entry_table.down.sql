-- Down: drop pos.pos_opening_entries table
DROP TABLE IF EXISTS pos.pos_opening_entries CASCADE;
DROP FUNCTION IF EXISTS pos.pos_opening_entries_audit_timestamp() CASCADE;
