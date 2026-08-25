-- Down: remove the company fence from the tables this migration fenced

-- Reverse the fence for pos.pos_manager_pins
DROP POLICY IF EXISTS pos_manager_pins_company_isolation ON pos.pos_manager_pins;
ALTER TABLE pos.pos_manager_pins NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_manager_pins DISABLE ROW LEVEL SECURITY;

-- Reverse the fence for pos.pos_floor_plans
DROP POLICY IF EXISTS pos_floor_plans_company_isolation ON pos.pos_floor_plans;
ALTER TABLE pos.pos_floor_plans NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_floor_plans DISABLE ROW LEVEL SECURITY;

-- Reverse the fence for pos.pos_tables
DROP POLICY IF EXISTS pos_tables_company_isolation ON pos.pos_tables;
ALTER TABLE pos.pos_tables NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_tables DISABLE ROW LEVEL SECURITY;

-- Reverse the fence for pos.pos_discounts
DROP POLICY IF EXISTS pos_discounts_company_isolation ON pos.pos_discounts;
ALTER TABLE pos.pos_discounts NO FORCE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_discounts DISABLE ROW LEVEL SECURITY;
