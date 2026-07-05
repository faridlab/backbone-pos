# BRD — backbone-pos

> Business Requirements & Rules. Tier 2 · Financials. Date: 2026-07-05. Pairs with
> `docs/business-flows/golden-cases.md`.

## Documents
PosProfile (register + GL accounts) · PosOpeningEntry (session, opening float per method) ·
PosInvoice (+items; the ticket) · PosPayment (tender) · PosClosingEntry (Z-report).

## Business rules
**BR-1 (server-side money + rounding).** `net_amount = money(qty·price) − discount`; `net_total = Σ`;
`grand_total = net_total + tax_total`; `rounded_total = round(grand, round_to)` (nearest step, IDR
receipt rounding, half-away-from-zero); `rounding_adjustment = rounded − grand`. 2dp.

**BR-2 (non-empty / non-negative / unique).** ≥1 line; no negative qty/price/discount; net line ≥ 0;
unique `receipt_number`. → `empty_document` / `negative_amount` / `duplicate_number`.

**BR-3 (session lifecycle).** A sale rings only against an **open** `PosOpeningEntry` (→
`session_not_open`); close flips the session to `closed`, after which no sale may ring against it.

**BR-4 (multi-tender + change).** Each `add_tender` recomputes `paid_total = Σ tenders` and
`change_due = max(0, paid_total − rounded_total)`; the ticket is fully tendered when `paid_total ≥
rounded_total`. Tenders are added only while the ticket is `draft` (→ `not_draft`).

**BR-5 (recognition — the retail seam, ADR-001).** `recognize_sale` requires a **fully-tendered**
`draft` ticket (→ `not_fully_tendered`) and the register's GL accounts (→ `missing_account`). It drives
billing (`BillingPort::raise_and_post` — the real Sales Invoice + revenue post) then payment
(`PaymentPort::settle` — the tender settlement), links `billing_invoice_id`, flips the ticket to
`paid`, and emits `PosInvoicePaid`. **POS posts no GL directly.** For a cash sale the two posts leave
the A/R control at **zero**. IDR-only for now; PPN is refused until a profile tax account exists.

**BR-6 (billing raised at most once — persist-and-reuse; council 2026-07-05).** The instant billing's
`raise_and_post` returns, POS persists `billing_invoice_id` on the still-`draft` ticket; any re-entry
**reuses** it and skips the raise. So an ordinary decline-then-retry (the two posts are a saga, not
atomic) does **not** book a second revenue journal (billing does not dedup on the POS ticket).
`recognize_sale` also short-circuits on `status='paid'`, and the link/event stay gated on the
`draft→paid` transition (once-only event). Parked: per-ticket concurrency lock, `settle` crash-window
idempotency (ADR-001).

**BR-7 (drawer reconciliation at close).** Per method: `expected = opening_float + Σ recognised
tenders` (cash also `− Σ change_due`); `difference = counted − expected`; `difference_total = Σ`. The
`PosClosingEntry` persists the per-method breakdown; over/short becomes a write-off (billing/accounting)
against the profile's `write_off_account_id` (deferred).

**BR-8 (returns — KEEP, ADR-001 §5; council 2026-07-05).** `return_sale` reverses BOTH legs: billing's
credit note (`reverse_sales_invoice` → `Dr Revenue · Cr A/R`, invoice → cancelled) AND payment's refund
(`reverse_payment` → `Dr A/R · Cr Cash`), so revenue, cash, and A/R all net to zero. Records an
`is_return` ticket linked via `return_against`, flips the original → `returned`, emits
`PosInvoiceReturned`. **Idempotent** (both reversals idempotent + `paid→returned` gate → refund/credit
at most once). → `not_returnable` if the sale was never recognised. Full-ticket only (partial parked).

## Events
`PosSessionOpened`, `PosInvoicePaid` (carries the billing invoice + payment), `PosSessionClosed`,
`PosInvoiceReturned` (the return — billing credit note + payment refund).

## Deferred (with reason)
Consolidated billing handoff, offline sync, PPN-on-receipt (billing/tax), fiscal printer, multi-currency,
over/short write-off automation, partial/line-level returns.
