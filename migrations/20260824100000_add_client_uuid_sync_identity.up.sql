-- Offline-sync identity: client_uuid on the ticket, its lines, and its tenders
-- A client that sells offline mints a UUID per order, per line, and per payment and
-- replays them to the server later; the server reconciles by these uuids (a replayed
-- row UPDATEs the row carrying its uuid instead of creating a duplicate). The column
-- is nullable — server-originated counter sales carry no client uuid — and it is
-- deliberately DISTINCT from the primary key: the PK stays server-owned identity,
-- the client_uuid is the trust-scoped reconciliation key (trusted for identity only;
-- every monetary total is recomputed server-side).
-- Uniqueness is partial on live rows: soft-deleting a row frees its uuid.
-- The uuid namespaces INSIDE a tenant: uniqueness is (company_id, client_uuid), so two companies
-- replaying the same device uuid each get their own ticket and never see each other's.
-- Column adds are nullable, so databases at the previous shape apply this unchanged.

ALTER TABLE pos.pos_invoices ADD COLUMN IF NOT EXISTS client_uuid UUID;
ALTER TABLE pos.pos_invoice_items ADD COLUMN IF NOT EXISTS client_uuid UUID;
ALTER TABLE pos.pos_payments ADD COLUMN IF NOT EXISTS client_uuid UUID;

CREATE UNIQUE INDEX IF NOT EXISTS idx_pos_invoices_client_uuid ON pos.pos_invoices (company_id, client_uuid) WHERE (metadata->>'deleted_at') IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_pos_invoice_items_client_uuid ON pos.pos_invoice_items (company_id, client_uuid) WHERE (metadata->>'deleted_at') IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_pos_payments_client_uuid ON pos.pos_payments (company_id, client_uuid) WHERE (metadata->>'deleted_at') IS NULL;
