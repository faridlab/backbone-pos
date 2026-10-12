-- Stock relief, recorded on the ticket: pos.pos_invoices.stock_delivery_note_id names the
-- delivery note that relieved a paid ticket's stock. Recognition issues the relief whenever a
-- paid, stock-tracking ticket has none, so a relief refused after the ticket was marked paid is
-- retried instead of skipped, and a ticket that has one is never relieved twice. Nullable:
-- non-stock registers never fill it, and databases at the previous shape apply this unchanged.

ALTER TABLE pos.pos_invoices ADD COLUMN IF NOT EXISTS stock_delivery_note_id UUID;
