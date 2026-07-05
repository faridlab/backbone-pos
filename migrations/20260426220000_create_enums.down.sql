-- Down: drop enum types for pos module
DROP TYPE IF EXISTS pos_session_status CASCADE;
DROP TYPE IF EXISTS pos_payment_method CASCADE;
DROP TYPE IF EXISTS pos_invoice_status CASCADE;
DROP TYPE IF EXISTS pos_closing_status CASCADE;
