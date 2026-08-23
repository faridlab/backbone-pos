-- Revert the ADR-0014 strict fence re-statement for pos module.
-- The fence predates this migration (ADR-0008-era; the two parent-scoped children
-- were fenced by the ADR-0010 Decision A migration), so the honest reverse is to
-- re-state the same live policy, not to disarm the tables: a down that disabled RLS
-- would leave company data unfenced — a posture this module never had.

-- Re-state the pre-existing fence for pos.pos_closing_entries (identical policy; see header).
DROP POLICY IF EXISTS pos_closing_entries_company_isolation ON pos.pos_closing_entries;
CREATE POLICY pos_closing_entries_company_isolation ON pos.pos_closing_entries
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- Re-state the pre-existing fence for pos.pos_invoices (identical policy; see header).
DROP POLICY IF EXISTS pos_invoices_company_isolation ON pos.pos_invoices;
CREATE POLICY pos_invoices_company_isolation ON pos.pos_invoices
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- Re-state the pre-existing fence for pos.pos_invoice_items (identical policy; see header).
DROP POLICY IF EXISTS pos_invoice_items_company_isolation ON pos.pos_invoice_items;
CREATE POLICY pos_invoice_items_company_isolation ON pos.pos_invoice_items
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- Re-state the pre-existing fence for pos.pos_payments (identical policy; see header).
DROP POLICY IF EXISTS pos_payments_company_isolation ON pos.pos_payments;
CREATE POLICY pos_payments_company_isolation ON pos.pos_payments
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- Re-state the pre-existing fence for pos.pos_profiles (identical policy; see header).
DROP POLICY IF EXISTS pos_profiles_company_isolation ON pos.pos_profiles;
CREATE POLICY pos_profiles_company_isolation ON pos.pos_profiles
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- Re-state the pre-existing fence for pos.pos_opening_entries (identical policy; see header).
DROP POLICY IF EXISTS pos_opening_entries_company_isolation ON pos.pos_opening_entries;
CREATE POLICY pos_opening_entries_company_isolation ON pos.pos_opening_entries
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- Re-state the pre-existing fence for pos.pos_cash_movements (identical policy; see header).
DROP POLICY IF EXISTS pos_cash_movements_company_isolation ON pos.pos_cash_movements;
CREATE POLICY pos_cash_movements_company_isolation ON pos.pos_cash_movements
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

