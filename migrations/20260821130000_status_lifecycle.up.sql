-- Migration: replace the pos-profile active boolean with a status enum
-- pos.pos_profiles carried `is_active BOOLEAN NOT NULL DEFAULT TRUE`; the tree-wide convention is
-- one `status` enum field per lifecycle (see docs/refactoring-schema in the serpa workspace).
-- FALSE rows are written to 'inactive'; TRUE rows ride the new column's DEFAULT 'active'
-- (no UPDATE needed). The enum type is created unqualified so it lands beside the module's other
-- enum types (public), where the generated sqlx type_name resolves.

DO $$ BEGIN
    CREATE TYPE pos_profile_status AS ENUM ('active', 'inactive');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

ALTER TABLE pos.pos_profiles ADD COLUMN status pos_profile_status NOT NULL DEFAULT 'active';
UPDATE pos.pos_profiles SET status = 'inactive' WHERE NOT is_active;
ALTER TABLE pos.pos_profiles DROP COLUMN is_active;

DROP INDEX IF EXISTS pos.idx_pos_profiles_company_id_is_active;
CREATE INDEX IF NOT EXISTS idx_pos_profiles_company_id_status ON pos.pos_profiles (company_id, status);
