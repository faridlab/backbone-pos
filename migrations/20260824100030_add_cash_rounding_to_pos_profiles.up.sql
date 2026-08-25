-- Cash rounding configuration on the register profile
-- A register can round the charged total to cash: strategy 'half_up' rounds the
-- grand total HALF-UP to the nearest multiple of cash_rounding_unit (a difference
-- of at most unit/2 is absorbed into the ticket's rounding_adjustment); strategy
-- 'none' charges the exact total. Both columns carry defaults, so databases at the
-- previous shape apply this unchanged and every existing register reads as
-- strategy 'none' with unit 0 (no rounding) — the behaviour such registers had
-- before the columns existed.
-- The enum type is created unqualified so it lands beside the module's other enum
-- types (public), where the generated sqlx type_name resolves.

DO $$ BEGIN
    CREATE TYPE pos_cash_rounding_strategy AS ENUM ('none', 'half_up');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

ALTER TABLE pos.pos_profiles ADD COLUMN IF NOT EXISTS cash_rounding_strategy pos_cash_rounding_strategy NOT NULL DEFAULT 'none';
ALTER TABLE pos.pos_profiles ADD COLUMN IF NOT EXISTS cash_rounding_unit NUMERIC(18, 2) NOT NULL DEFAULT 0 CHECK (cash_rounding_unit >= 0);
