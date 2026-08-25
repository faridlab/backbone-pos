-- Down: remove the offline-sync identity columns and their uniqueness

DROP INDEX IF EXISTS idx_pos_payments_client_uuid;
DROP INDEX IF EXISTS idx_pos_invoice_items_client_uuid;
DROP INDEX IF EXISTS idx_pos_invoices_client_uuid;

ALTER TABLE pos.pos_payments DROP COLUMN IF EXISTS client_uuid;
ALTER TABLE pos.pos_invoice_items DROP COLUMN IF EXISTS client_uuid;
ALTER TABLE pos.pos_invoices DROP COLUMN IF EXISTS client_uuid;
