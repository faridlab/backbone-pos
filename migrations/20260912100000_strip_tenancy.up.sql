-- Hand-authored (user-owned). Not regenerated.
--
-- Strip every company-fence artifact from the pos tables (ADR-0029): the module is
-- tenant-agnostic; org scoping is installed by the COMPOSING service's tenancy decorator,
-- never by the module. Dropped here, per table: the company-leading indexes and uniques,
-- the pos_<table>_company_isolation RLS policy, and the company_id column itself.
--
-- The company-led client_uuid partial uniques (invoices, invoice_items, payments) are
-- dropped too: their org-scoped re-declarations live in the composing service's tenancy
-- decorator, keyed on (org_unit_id, client_uuid) over the same live-row predicate. The
-- drifted one-open-session unique idx_pos_opening_entries_company_id_pos_profile_id is
-- dropped BY NAME (it was a re-key of the pos_profile_id-only unique, whose org-scoped
-- re-declaration likewise moves to the decorator).
--
-- Ordering guard (the decorator must run FIRST on any database with data): the module
-- never moves tenancy data. A table is safe to strip when EITHER
--   a) it carries org_unit_id with no NULLs — the decorator backfilled it from company_id —
--      or b) it is empty (a fresh database: the earlier chain files created it empty).
-- Otherwise the strip RAISEs, naming the decorator step, rather than dropping a column
-- that still holds the only tenancy key. The file is re-runnable (every drop is IF EXISTS
-- and the tracker has no checksums), so a failed run retries cleanly after the decorator
-- lands.
--
-- RLS enable/force flags are deliberately NOT touched: the decorator owns those now.
-- The multi-tenant outbox's events live in the COMPOSING service's schema, not here, so
-- this module keeps no outbox column of its own; its outbox writes fill the legacy
-- company echo from the ambient org scope.

DO $$
DECLARE
    t text;
    has_org boolean;
    org_nulls bigint;
    total bigint;
    offenders text := '';
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'pos_profiles', 'pos_floor_plans', 'pos_tables', 'pos_discounts',
        'pos_opening_entries', 'pos_closing_entries', 'pos_cash_movements',
        'pos_invoices', 'pos_invoice_items', 'pos_payments', 'pos_manager_pins'
    ]
    LOOP
        IF to_regclass(format('pos.%I', t)) IS NULL THEN
            CONTINUE; -- chain not fully applied on this database; nothing to strip
        END IF;

        SELECT EXISTS (
                   SELECT 1 FROM information_schema.columns
                   WHERE table_schema = 'pos' AND table_name = t AND column_name = 'org_unit_id'
               )
        INTO has_org;

        EXECUTE format('SELECT count(*) FROM pos.%I', t) INTO total;

        IF has_org THEN
            EXECUTE format(
                'SELECT count(*) FROM pos.%I WHERE org_unit_id IS NULL', t)
            INTO org_nulls;
        ELSE
            org_nulls := total; -- no org column: every row's only tenancy key is company_id
        END IF;

        IF has_org AND org_nulls = 0 THEN
            CONTINUE; -- decorator backfilled: safe
        END IF;
        IF total = 0 THEN
            CONTINUE; -- empty table (fresh database): safe
        END IF;
        offenders := offenders || format(' pos.%s (%s rows, %s rows not covered by org_unit_id);', t, total, org_nulls);
    END LOOP;

    IF offenders <> '' THEN
        RAISE EXCEPTION 'refusing to strip company_id — these tables are not yet covered by the tenancy decorator:%. Apply the composing service''s tenancy decorator (it backfills org_unit_id from company_id) and re-run; it is the only step that moves tenancy data.', offenders;
    END IF;
END $$;

-- ── pos_profiles ───────────────────────────────────────────────────────────────
DROP INDEX IF EXISTS pos.idx_pos_profiles_company_id_status;
DROP POLICY IF EXISTS pos_profiles_company_isolation ON pos.pos_profiles;
ALTER TABLE pos.pos_profiles DROP COLUMN IF EXISTS company_id;

-- ── pos_floor_plans ────────────────────────────────────────────────────────────
DROP INDEX IF EXISTS pos.idx_pos_floor_plans_company_id_branch_id;
DROP POLICY IF EXISTS pos_floor_plans_company_isolation ON pos.pos_floor_plans;
ALTER TABLE pos.pos_floor_plans DROP COLUMN IF EXISTS company_id;

-- ── pos_tables ─────────────────────────────────────────────────────────────────
DROP POLICY IF EXISTS pos_tables_company_isolation ON pos.pos_tables;
ALTER TABLE pos.pos_tables DROP COLUMN IF EXISTS company_id;

-- ── pos_discounts ──────────────────────────────────────────────────────────────
DROP INDEX IF EXISTS pos.idx_pos_discounts_company_id;
DROP POLICY IF EXISTS pos_discounts_company_isolation ON pos.pos_discounts;
ALTER TABLE pos.pos_discounts DROP COLUMN IF EXISTS company_id;

-- ── pos_opening_entries ────────────────────────────────────────────────────────
DROP INDEX IF EXISTS pos.idx_pos_opening_entries_company_id_pos_profile_id_status;
DROP INDEX IF EXISTS pos.idx_pos_opening_entries_company_id_pos_profile_id;
DROP POLICY IF EXISTS pos_opening_entries_company_isolation ON pos.pos_opening_entries;
ALTER TABLE pos.pos_opening_entries DROP COLUMN IF EXISTS company_id;

-- ── pos_closing_entries ────────────────────────────────────────────────────────
DROP INDEX IF EXISTS pos.idx_pos_closing_entries_company_id_pos_profile_id_status;
DROP POLICY IF EXISTS pos_closing_entries_company_isolation ON pos.pos_closing_entries;
ALTER TABLE pos.pos_closing_entries DROP COLUMN IF EXISTS company_id;

-- ── pos_cash_movements ─────────────────────────────────────────────────────────
DROP INDEX IF EXISTS pos.idx_pos_cash_movements_company_id_pos_profile_id;
DROP POLICY IF EXISTS pos_cash_movements_company_isolation ON pos.pos_cash_movements;
ALTER TABLE pos.pos_cash_movements DROP COLUMN IF EXISTS company_id;

-- ── pos_invoices ───────────────────────────────────────────────────────────────
DROP INDEX IF EXISTS pos.idx_pos_invoices_company_id_opening_entry_id_status;
DROP INDEX IF EXISTS pos.idx_pos_invoices_client_uuid;
DROP POLICY IF EXISTS pos_invoices_company_isolation ON pos.pos_invoices;
ALTER TABLE pos.pos_invoices DROP COLUMN IF EXISTS company_id;

-- ── pos_invoice_items ──────────────────────────────────────────────────────────
DROP INDEX IF EXISTS pos.idx_pos_invoice_items_company_id;
DROP INDEX IF EXISTS pos.idx_pos_invoice_items_client_uuid;
DROP POLICY IF EXISTS pos_invoice_items_company_isolation ON pos.pos_invoice_items;
ALTER TABLE pos.pos_invoice_items DROP COLUMN IF EXISTS company_id;

-- ── pos_payments ───────────────────────────────────────────────────────────────
DROP INDEX IF EXISTS pos.idx_pos_payments_company_id;
DROP INDEX IF EXISTS pos.idx_pos_payments_client_uuid;
DROP POLICY IF EXISTS pos_payments_company_isolation ON pos.pos_payments;
ALTER TABLE pos.pos_payments DROP COLUMN IF EXISTS company_id;

-- ── pos_manager_pins ───────────────────────────────────────────────────────────
DROP INDEX IF EXISTS pos.idx_pos_manager_pins_company_id_employee_party_id;
DROP POLICY IF EXISTS pos_manager_pins_company_isolation ON pos.pos_manager_pins;
ALTER TABLE pos.pos_manager_pins DROP COLUMN IF EXISTS company_id;
