-- Down: restore the register-slot unique keyed on pos_profile_id alone (global across
-- tenants). If open session rows sharing one register uuid across two tenants exist,
-- the global unique cannot be rebuilt — resolve those rows before rolling back.

DROP INDEX IF EXISTS pos.idx_pos_opening_entries_company_id_pos_profile_id;

CREATE UNIQUE INDEX IF NOT EXISTS idx_pos_opening_entries_pos_profile_id
    ON pos.pos_opening_entries (pos_profile_id)
    WHERE status = 'open' AND (metadata->>'deleted_at') IS NULL;
