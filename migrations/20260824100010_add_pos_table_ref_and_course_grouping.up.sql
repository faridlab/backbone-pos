-- Restaurant seating: table ref on the ticket, course grouping on the line
-- pos.pos_invoices.pos_table_id names the dining table a ticket is open on
-- (NULL for counter sales); pos.pos_invoice_items.course groups lines that fire
-- together in the kitchen. Both adds are nullable, so databases at the previous
-- shape apply this unchanged. The partial unique enforces one DRAFT ticket per
-- table: a register that opens a second draft on a table already holding one is
-- refused at the database fence as well as by the service — the draft must be
-- resumed or moved, never forked. Counter-service NULL tables are excluded, and
-- the constraint relaxes the moment a draft leaves the draft state.

ALTER TABLE pos.pos_invoices ADD COLUMN IF NOT EXISTS pos_table_id UUID;
ALTER TABLE pos.pos_invoice_items ADD COLUMN IF NOT EXISTS course INTEGER;

CREATE UNIQUE INDEX IF NOT EXISTS idx_pos_invoices_pos_table_id ON pos.pos_invoices (pos_table_id) WHERE status = 'draft' AND pos_table_id IS NOT NULL AND (metadata->>'deleted_at') IS NULL;
