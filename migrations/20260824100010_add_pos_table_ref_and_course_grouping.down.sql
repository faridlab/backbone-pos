-- Down: remove the table ref, the course grouping, and the one-draft-per-table rule

DROP INDEX IF EXISTS idx_pos_invoices_pos_table_id;

ALTER TABLE pos.pos_invoice_items DROP COLUMN IF EXISTS course;
ALTER TABLE pos.pos_invoices DROP COLUMN IF EXISTS pos_table_id;
