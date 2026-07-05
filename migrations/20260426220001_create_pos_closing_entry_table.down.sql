-- Down: drop pos.pos_closing_entries table
DROP TABLE IF EXISTS pos.pos_closing_entries CASCADE;
DROP FUNCTION IF EXISTS pos.pos_closing_entries_audit_timestamp() CASCADE;
