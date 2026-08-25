-- Down: remove the cash rounding configuration from the register profile

ALTER TABLE pos.pos_profiles DROP COLUMN IF EXISTS cash_rounding_unit;
ALTER TABLE pos.pos_profiles DROP COLUMN IF EXISTS cash_rounding_strategy;

DROP TYPE IF EXISTS pos_cash_rounding_strategy;
