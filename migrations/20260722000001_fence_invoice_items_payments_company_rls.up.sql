-- Migration: add direct company_id + FORCE RLS fence to pos.pos_invoice_items and pos.pos_payments
-- (ADR-0010 Decision A). Previously these children were unfenced (pos review F1): the parent
-- pos_invoices carried company_id + RLS, but a cross-tenant SELECT on the children joined through
-- pos_invoice_id would slip past the fence. This denormalises company_id onto each child row and
-- applies the standard ADR-0008 invariant #1 policy, identical to every other pos table.
--
-- Backbone convention: company_id is a LOGICAL FK only (organization.Company.id) — no DB-level
-- REFERENCES constraint, matching PosInvoice.company_id and every other @exclude_from_foreign_key_check
-- column in the module.

-- ---------------------------------------------------------------------------
-- pos.pos_invoice_items
-- ---------------------------------------------------------------------------

-- 1. Add the column nullable so the backfill can proceed without a rewrite.
ALTER TABLE pos.pos_invoice_items ADD COLUMN IF NOT EXISTS company_id UUID;

-- 2. Backfill from the parent ticket. pos_invoice_id is the unique, unambiguous FK to pos_invoices.id
--    (sole FK declared on this table in the schema SSoT).
UPDATE pos.pos_invoice_items AS li
   SET company_id = i.company_id
  FROM pos.pos_invoices AS i
 WHERE i.id = li.pos_invoice_id
   AND li.company_id IS NULL;

-- 3. NOT NULL now that every row carries its owner. If a stray orphan exists (no parent ticket), this
--    fails loud — orphans must be cleaned up by hand, not silently papered over.
ALTER TABLE pos.pos_invoice_items ALTER COLUMN company_id SET NOT NULL;

-- 4. Index the fence column (every other fenced pos table does the same).
CREATE INDEX IF NOT EXISTS idx_pos_invoice_items_company_id ON pos.pos_invoice_items (company_id);

-- 5. ADR-0008 invariant #1: ENABLE + FORCE, then the standard company-isolation policy. FORCE is
--    load-bearing — without it the table owner (migrations, seeders) would bypass the fence and a
--    later admin-connection write could land a row the policy would have rejected.
ALTER TABLE pos.pos_invoice_items ENABLE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_invoice_items FORCE  ROW LEVEL SECURITY;
DROP POLICY IF EXISTS pos_invoice_items_company_isolation ON pos.pos_invoice_items;
CREATE POLICY pos_invoice_items_company_isolation ON pos.pos_invoice_items
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- ---------------------------------------------------------------------------
-- pos.pos_payments
-- ---------------------------------------------------------------------------

ALTER TABLE pos.pos_payments ADD COLUMN IF NOT EXISTS company_id UUID;

UPDATE pos.pos_payments AS pay
   SET company_id = i.company_id
  FROM pos.pos_invoices AS i
 WHERE i.id = pay.pos_invoice_id
   AND pay.company_id IS NULL;

ALTER TABLE pos.pos_payments ALTER COLUMN company_id SET NOT NULL;

CREATE INDEX IF NOT EXISTS idx_pos_payments_company_id ON pos.pos_payments (company_id);

ALTER TABLE pos.pos_payments ENABLE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_payments FORCE  ROW LEVEL SECURITY;
DROP POLICY IF EXISTS pos_payments_company_isolation ON pos.pos_payments;
CREATE POLICY pos_payments_company_isolation ON pos.pos_payments
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
