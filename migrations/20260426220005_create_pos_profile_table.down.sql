-- Down: drop pos.pos_profiles table
DROP TABLE IF EXISTS pos.pos_profiles CASCADE;
DROP FUNCTION IF EXISTS pos.pos_profiles_audit_timestamp() CASCADE;
