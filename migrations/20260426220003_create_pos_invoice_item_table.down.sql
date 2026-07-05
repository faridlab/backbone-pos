-- Down: drop pos.pos_invoice_items table
DROP TABLE IF EXISTS pos.pos_invoice_items CASCADE;
DROP FUNCTION IF EXISTS pos.pos_invoice_items_audit_timestamp() CASCADE;
