# FSD — backbone-pos

> Functional Spec. Tier 2 · Financials. Date: 2026-07-05.

## Entities (schema/models/*.model.yaml — SSoT)
PosProfile (register; `receivable`/`income`/`cash`/`write_off` account refs) · PosOpeningEntry
(session; `opening_balances` json per method) · PosInvoice(+PosInvoiceItem) (net/tax/grand/rounded,
`change_due`, `billing_invoice_id`) · PosPayment (tender; `payment_method`, `payment_entry_id`) ·
PosClosingEntry (`totals_by_method` json, `difference_total`). Cross-module ids are logical FKs
(`@exclude_from_foreign_key_check`): account→accounting, `billing_invoice_id`→billing,
`payment_entry_id`→payment, customer→party, company/branch→organization, item→catalog.

## Services (application/service — hand-authored, user_owned)
- `PosWriteService` — `open_session`; `ring_sale` (server-side money + IDR rounding); `add_tender`
  (paid_total + change_due, draft-only); `recognize_sale` (the seam: guard fully-tendered → drive
  `BillingPort` + `PaymentPort` → link + `draft→paid` gate → publish `PosInvoicePaid`; idempotent);
  `close_session` (per-method expected-vs-counted reconciliation + session close).
- `pos_ports` — the outbound orchestration ports: `BillingPort::raise_and_post(SaleInvoiceRequest) ->
  InvoiceAck`, `PaymentPort::settle(SettlementRequest) -> SettlementAck`. POS drives billing + payment
  through these; **zero normal Cargo edge** (a composition implements them over the real services).
- `pos_events` — `PosEvent` {`PosSessionOpened`, `PosInvoicePaid` (billing invoice + payment),
  `PosSessionClosed`} + `PosEventSink`.

## HTTP surface (presentation/http/guarded_routes.rs)
`create_guarded_pos_routes(&PosModule, pool)` — read documents + validated `POST /pos-sessions` /
`/pos-sales` / `/pos-tenders` / `/pos-sessions/close`. No generic mutation. `recognize_sale` needs the
ports, so it is service/job-driven.

## State machines
- Session (`PosSessionStatus`): `open → closed`.
- Ticket (`PosInvoiceStatus`): `draft → paid` (recognised) / `void`; `consolidated`/`returned` deferred.
- Close (`PosClosingStatus`): `draft → submitted` (→ `reconciled` when over/short is written off).

## Integration seams
- **Retail-sale seam (proven, marquee):** `recognize_sale` → `BillingPort` (billing raises + posts the
  Sales Invoice — `Dr A/R · Cr Revenue`) → `PaymentPort` (payment settles the tender — `Dr Cash · Cr
  A/R` + billing `apply_settlement`) → the **A/R control nets to zero**, cash holds the takings, ticket
  `paid`, `PosInvoicePaid` emitted. Across POS + billing + payment + accounting, zero normal Cargo edge.
  ADR-001, `tests/retail_sale_seam.rs`, `scripts/retail_sale_seam_roundtrip.sh`.
- **Outbound:** `PosInvoicePaid` → loyalty accrual / retail analytics. **Inbound (future):** promo's
  resolved prices (input), returns → billing credit note + payment refund.

## Test oracle
`pos_golden_cases` (5: ring totals, IDR rounding up/down, multi-tender + change, close reconciliation,
validation gates), `integrity_probes` (4: recognition-requires-full-tender, requires-register-accounts,
recognition idempotent, **IP-4 billing raised at-most-once across decline-then-retry** — council
2026-07-05), `retail_sale_seam` (2: RSSEAM-1 A/R nets to zero across four modules; **RSSEAM-2 return
reverses both legs — revenue/cash/A/R all net to zero, idempotent** — council 2026-07-05; + §5).
**11 tests** (of the hand-authored suite).
