-- Reverse the tax template refs on the register.
ALTER TABLE pos.pos_profiles DROP COLUMN IF EXISTS tax_template_ids;
