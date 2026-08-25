-- Scope the one-open-session-per-register unique to the tenant.
-- The prior unique keyed on pos_profile_id alone, which is global across tenants: an
-- open session row carrying a register uuid the tenant does not own (a stale client
-- reference, or any writer that did not first validate the profile against its own
-- tenant) occupies the register's slot, and the rightful tenant's open is then refused
-- with the one-open-session violation. The register-slot invariant belongs to the
-- tenant that owns the register: re-key it on (company_id, pos_profile_id) over the
-- same live-row predicate (status 'open', not soft-deleted).
-- The new index name keeps the pos_profile_id column fragment so the service's
-- unique-violation mapping by constraint name still recognises it.
-- Databases that already applied the earlier migration re-key here; fresh databases
-- create the global index first and immediately re-key it in the same run.

DROP INDEX IF EXISTS pos.idx_pos_opening_entries_pos_profile_id;

CREATE UNIQUE INDEX IF NOT EXISTS idx_pos_opening_entries_company_id_pos_profile_id
    ON pos.pos_opening_entries (company_id, pos_profile_id)
    WHERE status = 'open' AND (metadata->>'deleted_at') IS NULL;
