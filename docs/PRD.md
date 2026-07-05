# PRD — backbone-pos

> Tier 2 · Financials · Indonesia-first ERP. Status: built. Date: 2026-07-05.

## Problem & intent
The in-store counter needs to ring a sale, take mixed tender, and close the drawer — fast, and without
re-implementing revenue/tax recognition or settlement. `backbone-pos` owns the **cashier session +
ticket** (the BERSIHir retail path) and **orchestrates** the sale: on recognition it drives
`backbone-billing` (raise + post the real Sales Invoice — revenue) and `backbone-payment` (settle the
tender), so retail reuses the same GL emitters as web/B2B sales. **POS posts no GL itself.**

## Goals
- Own **PosProfile** (register config + the GL accounts the handoff needs), **PosOpeningEntry** (till
  open with opening float per method), **PosInvoice**(+items) + **PosPayment** (tender), **PosClosingEntry**
  (Z-report drawer reconciliation).
- Compute money **server-side**; IDR receipt **rounding**; guarded surface (no generic mutation).
- **Multi-tender** (cash/card/QRIS/e-wallet/…) with **change due** on cash overpayment.
- **Recognise** a fully-tendered sale through the `BillingPort`/`PaymentPort` seam: a cash sale books
  `Dr A/R · Cr Revenue` (billing) then `Dr Cash · Cr A/R` (payment) — **A/R nets to zero at the
  counter**. Idempotent.
- **Close** the session: expected (opening float + Σ recognised tenders, cash − change) vs counted per
  method → over/short.

## Non-goals (this phase / deferred)
Returns / voids (credit note + refund via billing/payment), consolidated (batched) billing handoff,
offline sync conflict resolution, PPN on the receipt (delegated to billing/tax — the base is tax-free
until a profile tax account is configured), fiscal-printer integrations, POS-specific promo engine
(reuse backbone-promo's resolved prices), multi-currency POS.

## Personas
Cashier (opens till, rings sales, takes tender, closes drawer), Store manager (reconciles Z-reports),
Integrating engineer (implements the billing/payment ports, subscribes to `PosInvoicePaid` for loyalty
/ analytics).

## Success criteria
- Sale math + rounding + multi-tender + close reconciliation locked by a numeric oracle (5 golden) +
  integrity probes (3, incl. recognition-requires-full-tender + idempotency).
- The retail-sale seam proven end-to-end against the real ledger (RSSEAM-1: **A/R nets to zero** across
  POS + billing + payment + accounting) + survives regen of all three modules (§5).
- Indonesia-ready: QRIS + e-wallet tender methods; IDR rounding; PPN delegated to billing/tax.
