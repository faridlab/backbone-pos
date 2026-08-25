-- Company fence posture for the new pos tables (ADR-0014: strict)
-- The four tables created immediately before this migration — pos.pos_manager_pins,
-- pos.pos_floor_plans, pos.pos_tables, pos.pos_discounts — carry company_id and are
-- fenced strict like every other pos table: a session sees only rows whose
-- company_id equals the request-scoped company (`set_config('app.company_id',
-- <uuid>, true)`); an unset var sees zero rows (fail-closed). The module-level
-- RLS migration re-states policies for tables that existed when it shipped, so
-- tables added later get their fence here; this file is hand-maintained and
-- listed under user_owned in metaphor.codegen.yaml so a forced regen cannot
-- drop it. Requires the app to connect as a non-superuser role; migrations and
-- seeders run as the owner and bypass.

-- Fence for pos.pos_manager_pins
ALTER TABLE pos.pos_manager_pins ENABLE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_manager_pins FORCE  ROW LEVEL SECURITY;
DROP POLICY IF EXISTS pos_manager_pins_company_isolation ON pos.pos_manager_pins;
CREATE POLICY pos_manager_pins_company_isolation ON pos.pos_manager_pins
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- Fence for pos.pos_floor_plans
ALTER TABLE pos.pos_floor_plans ENABLE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_floor_plans FORCE  ROW LEVEL SECURITY;
DROP POLICY IF EXISTS pos_floor_plans_company_isolation ON pos.pos_floor_plans;
CREATE POLICY pos_floor_plans_company_isolation ON pos.pos_floor_plans
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- Fence for pos.pos_tables
ALTER TABLE pos.pos_tables ENABLE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_tables FORCE  ROW LEVEL SECURITY;
DROP POLICY IF EXISTS pos_tables_company_isolation ON pos.pos_tables;
CREATE POLICY pos_tables_company_isolation ON pos.pos_tables
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- Fence for pos.pos_discounts
ALTER TABLE pos.pos_discounts ENABLE ROW LEVEL SECURITY;
ALTER TABLE pos.pos_discounts FORCE  ROW LEVEL SECURITY;
DROP POLICY IF EXISTS pos_discounts_company_isolation ON pos.pos_discounts;
CREATE POLICY pos_discounts_company_isolation ON pos.pos_discounts
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
