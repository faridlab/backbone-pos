-- Down: drop pos.pos_invoices table
DROP TABLE IF EXISTS pos.pos_invoices CASCADE;
DROP FUNCTION IF EXISTS pos.pos_invoices_audit_timestamp() CASCADE;
