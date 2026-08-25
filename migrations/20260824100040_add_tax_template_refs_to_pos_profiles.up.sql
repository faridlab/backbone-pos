-- Tax templates on the register: the refs the document-grade tax compute resolves
-- through (the composing service implements POS's PosTaxComputePort over the tax
-- engine's calculate_document). NULL/empty means the register is not tax-configured
-- and ringing refuses; a zero-rated template is the way to express a non-PKP
-- register. The flat tax_rate column stays for historical receipts only.
ALTER TABLE pos.pos_profiles ADD COLUMN tax_template_ids JSONB;
