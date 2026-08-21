-- Down: restore the pos-profile active boolean from the status enum
-- Only 'inactive' rows are written back to FALSE; 'active' rows ride the boolean DEFAULT TRUE.

ALTER TABLE pos.pos_profiles ADD COLUMN is_active BOOLEAN NOT NULL DEFAULT TRUE;
UPDATE pos.pos_profiles SET is_active = FALSE WHERE status = 'inactive';
ALTER TABLE pos.pos_profiles DROP COLUMN status;

DROP INDEX IF EXISTS idx_pos_profiles_company_id_status;
CREATE INDEX IF NOT EXISTS idx_pos_profiles_company_id_is_active ON pos.pos_profiles (company_id, is_active);

DROP TYPE IF EXISTS pos_profile_status;
