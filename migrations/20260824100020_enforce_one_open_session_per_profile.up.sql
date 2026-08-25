-- One non-closed cashier session per register
-- A register (pos_profile) may hold exactly one session with status 'open'.
-- Opening a second session on a register whose previous session never closed must
-- be refused — a forked session double-counts the drawer at close. The service
-- checks this; the partial unique makes the invariant hold even against a direct
-- write. Closed sessions are excluded, and soft-deleted rows do not occupy the slot.

CREATE UNIQUE INDEX IF NOT EXISTS idx_pos_opening_entries_pos_profile_id ON pos.pos_opening_entries (pos_profile_id) WHERE status = 'open' AND (metadata->>'deleted_at') IS NULL;
