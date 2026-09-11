-- Hand-authored (user-owned). Not regenerated.
--
-- Best-effort restore sketch for the tenancy strip (ADR-0029). This is a breaking module
-- release against dev-stage databases: the down re-adds the company_id column as nullable
-- with its plain indexes and the company isolation policy shape, but restores NO data —
-- rows written after the strip (or after the decorator re-keyed them) carry org_unit_id
-- only. The composing service's tenancy decorator remains the live fence; treat this
-- down as a schema-shape sketch for archaeology, not a usable rollback.
--
-- The org-scoped re-declarations (client_uuid partial uniques, the one-open-session
-- unique) are NOT restored here either: they were never this module's post-strip shape.

ALTER TABLE pos.pos_profiles         ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE pos.pos_floor_plans      ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE pos.pos_tables           ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE pos.pos_discounts        ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE pos.pos_opening_entries  ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE pos.pos_closing_entries  ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE pos.pos_cash_movements   ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE pos.pos_invoices         ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE pos.pos_invoice_items    ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE pos.pos_payments         ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE pos.pos_manager_pins     ADD COLUMN IF NOT EXISTS company_id uuid;

CREATE INDEX IF NOT EXISTS idx_pos_profiles_company_id_status
    ON pos.pos_profiles (company_id, status);
CREATE INDEX IF NOT EXISTS idx_pos_floor_plans_company_id_branch_id
    ON pos.pos_floor_plans (company_id, branch_id);
CREATE INDEX IF NOT EXISTS idx_pos_discounts_company_id
    ON pos.pos_discounts (company_id);
CREATE INDEX IF NOT EXISTS idx_pos_opening_entries_company_id_pos_profile_id_status
    ON pos.pos_opening_entries (company_id, pos_profile_id, status);
CREATE INDEX IF NOT EXISTS idx_pos_closing_entries_company_id_pos_profile_id_status
    ON pos.pos_closing_entries (company_id, pos_profile_id, status);
CREATE INDEX IF NOT EXISTS idx_pos_cash_movements_company_id_pos_profile_id
    ON pos.pos_cash_movements (company_id, pos_profile_id);
CREATE INDEX IF NOT EXISTS idx_pos_invoices_company_id_opening_entry_id_status
    ON pos.pos_invoices (company_id, opening_entry_id, status);
CREATE INDEX IF NOT EXISTS idx_pos_invoice_items_company_id
    ON pos.pos_invoice_items (company_id);
CREATE INDEX IF NOT EXISTS idx_pos_payments_company_id
    ON pos.pos_payments (company_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_pos_manager_pins_company_id_employee_party_id
    ON pos.pos_manager_pins (company_id, employee_party_id);

CREATE POLICY pos_profiles_company_isolation ON pos.pos_profiles
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY pos_floor_plans_company_isolation ON pos.pos_floor_plans
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY pos_tables_company_isolation ON pos.pos_tables
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY pos_discounts_company_isolation ON pos.pos_discounts
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY pos_opening_entries_company_isolation ON pos.pos_opening_entries
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY pos_closing_entries_company_isolation ON pos.pos_closing_entries
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY pos_cash_movements_company_isolation ON pos.pos_cash_movements
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY pos_invoices_company_isolation ON pos.pos_invoices
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY pos_invoice_items_company_isolation ON pos.pos_invoice_items
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY pos_payments_company_isolation ON pos.pos_payments
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
CREATE POLICY pos_manager_pins_company_isolation ON pos.pos_manager_pins
    FOR ALL USING (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
