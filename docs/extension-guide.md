# Extension Guide — backbone-pos

> Public contract per `docs/erp/extension-contract.md`. Stable path:
> `backbone_pos::application::service::*` (the generated `exports/` tree is unwired scaffolding).

## Public surface
**A. Domain events** (`pos_events`, the 3-variant `PosEvent`): `PosSessionOpened`, `PosInvoicePaid`
{pos_invoice_id, company_id, grand_total, rounded_total, billing_invoice_id, payment_id},
`PosSessionClosed` {closing_entry_id, opening_entry_id, difference_total}.

**B. The orchestration ports** (`pos_ports`) — POS drives billing + payment through these; a consumer
implements them over the real services:
- `BillingPort::raise_and_post(SaleInvoiceRequest) -> InvoiceAck` (over billing's create + post Sales
  Invoice — revenue).
- `PaymentPort::settle(SettlementRequest) -> SettlementAck` (over payment's create + post + billing's
  `apply_settlement` — tender settlement).
POS never imports billing/payment; the requests/acks are the wire contract.

## How a consumer extends
1. **Wire the retail seam** — implement `BillingPort` + `PaymentPort` in your composition crate,
   mapping the requests into billing/payment; pass them to `recognize_sale`. (Reference ACL:
   `tests/retail_sale_seam.rs`.)
2. **React to a paid sale** — subscribe to `PosInvoicePaid` (via a `PosEventSink` passed to
   `with_sink`) for loyalty accrual, receipt printing (statutory format off the event), retail analytics.
3. **Configure the register** — set the profile's `receivable`/`income`/`cash` accounts + a default
   walk-in customer; supply promo-resolved prices at `ring_sale`.
4. Keep logic in `user_owned`/`*_custom.rs` — survives regen (proven by
   `scripts/retail_sale_seam_roundtrip.sh`).

## Bounded-context split (important)
POS owns the **session + ticket + tender + drawer**; it posts **no GL**. Revenue/tax is billing's;
settlement is payment's. A cash sale nets A/R to zero because the two emitters do their own posts —
POS only sequences them and records the linkage.

## Not a contract
Generated CRUD events; internal session state; `// <<< CUSTOM` blocks (own edits only).

## Deferred surfaces
Returns/voids (credit note + refund), consolidated billing handoff, offline sync, PPN-on-receipt,
fiscal printer, multi-currency, over/short write-off — additive when built.
