-- Down: remove the company RLS fence for pos module

-- Reverse the company RLS fence for pos.pos_closing_entries
DROP POLICY IF EXISTS pos_closing_entries_company_isolation ON pos.pos_closing_entries;
ALTER TABLE pos.pos_closing_entries NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_closing_entries DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for pos.pos_invoices
DROP POLICY IF EXISTS pos_invoices_company_isolation ON pos.pos_invoices;
ALTER TABLE pos.pos_invoices NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_invoices DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for pos.pos_profiles
DROP POLICY IF EXISTS pos_profiles_company_isolation ON pos.pos_profiles;
ALTER TABLE pos.pos_profiles NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_profiles DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for pos.pos_opening_entries
DROP POLICY IF EXISTS pos_opening_entries_company_isolation ON pos.pos_opening_entries;
ALTER TABLE pos.pos_opening_entries NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_opening_entries DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for pos.pos_cash_movements
DROP POLICY IF EXISTS pos_cash_movements_company_isolation ON pos.pos_cash_movements;
ALTER TABLE pos.pos_cash_movements NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_cash_movements DISABLE ROW LEVEL SECURITY;

