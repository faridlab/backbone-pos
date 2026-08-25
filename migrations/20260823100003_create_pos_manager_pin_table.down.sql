-- Down: drop pos.pos_manager_pins table
DROP TABLE IF EXISTS pos.pos_manager_pins CASCADE;
DROP FUNCTION IF EXISTS pos.pos_manager_pins_audit_timestamp() CASCADE;
