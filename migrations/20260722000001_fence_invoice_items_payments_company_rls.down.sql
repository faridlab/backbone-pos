-- Down: reverse the company RLS fence + company_id column added to pos.pos_invoice_items and
-- pos.pos_payments by ADR-0010 Decision A. Restores the children to their pre-fence shape.

-- ---------------------------------------------------------------------------
-- pos.pos_payments
-- ---------------------------------------------------------------------------
DROP POLICY IF EXISTS pos_payments_company_isolation ON pos.pos_payments;
ALTER TABLE pos.pos_payments NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_payments DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS pos.idx_pos_payments_company_id;
ALTER TABLE pos.pos_payments DROP COLUMN IF EXISTS company_id;

-- ---------------------------------------------------------------------------
-- pos.pos_invoice_items
-- ---------------------------------------------------------------------------
DROP POLICY IF EXISTS pos_invoice_items_company_isolation ON pos.pos_invoice_items;
ALTER TABLE pos.pos_invoice_items NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_invoice_items DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS pos.idx_pos_invoice_items_company_id;
ALTER TABLE pos.pos_invoice_items DROP COLUMN IF EXISTS company_id;
