# ADR-001: POS owns the session + ticket; it posts no GL, orchestrating billing + payment (retail seam)

**Status**: Accepted — Applied 2026-07-05 (proven end-to-end)
**Deciders**: Farid (owner), build session 2026-07-05
**Related**: `docs/erp/financials.md`, `docs/erp/gl-posting-contract.md`,
`docs/erp/modules/backbone-pos.md`, billing ADR-001/002, payment ADR-001/002, `extension-contract.md` §5

## Context

`backbone-pos` is the retail counter path of the Financials pillar. A store sale must recognise revenue
(and tax) and settle tender — but those are billing's and payment's jobs. POS owns the cashier session
lifecycle + the ticket + tender + drawer, and **orchestrates** the two emitters rather than opening a
parallel posting path. This keeps the GL contract singular: retail uses the same billing + payment
posts as web/B2B sales.

## Decision

1. **Five entities, one recognition shape.** PosProfile (register + the GL accounts the handoff needs),
   PosOpeningEntry (session), PosInvoice(+items) + PosPayment (tender), PosClosingEntry (Z-report). Money
   is server-side (`net = money(qty·price) − discount`, IDR receipt rounding to the nearest step);
   generic CRUD is not mounted on the guarded surface.
2. **POS posts NO GL — it drives billing + payment through ports.** `recognize_sale` requires a
   fully-tendered `draft` ticket + the register's accounts, then calls `BillingPort::raise_and_post`
   (billing raises + posts the real Sales Invoice — `Dr A/R · Cr Revenue`) and `PaymentPort::settle`
   (payment posts the tender — `Dr Cash · Cr A/R` — and billing draws the invoice down). For a **cash
   sale the A/R control nets to zero at the counter**. The ports are the wire contract; the shipped POS
   library has **zero normal Cargo edge** to billing/payment/accounting (dev-deps only for the seam
   test). This is the envelope+ACL discipline every seam uses, here for *two* downstream emitters.
3. **Recognition raises billing AT MOST ONCE — persist-and-reuse (council 2026-07-05).** The two posts
   are a saga, not one atomic unit: billing commits its revenue GL *before* payment settles. So the
   instant `raise_and_post` returns, POS persists `billing_invoice_id` on the still-`draft` ticket; on
   any re-entry it **reuses** that invoice and skips the raise. Without this, an ordinary decline-then-
   retry (card declined / period closed) would re-drive billing — which does **not** dedup on the POS
   ticket (fresh `invoice_number`, `source_so_id` non-unique) — booking a **second revenue journal for
   one sale**. `recognize_sale` also short-circuits on `status='paid'`, and the link/event stay gated on
   the `draft→paid` transition (once-only event). Proven by **IP-4** (a tender that fails then retries →
   billing raised exactly once; fails against an always-raise).
4. **Multi-tender + change + drawer reconciliation.** Each tender recomputes `paid_total` +
   `change_due` (cash overpayment); close computes per-method `expected = opening_float + Σ recognised
   tenders` (cash − change) vs `counted` → over/short. Only IDR; PPN is refused until a profile tax
   account exists (delegated to billing/tax).
5. **Returns reverse BOTH legs (KEEP; completeness council 2026-07-05).** `return_sale` drives billing's
   **credit note** (`reverse_sales_invoice` — `Dr Revenue · Cr A/R`, invoice → cancelled; a NEW billing
   method — `reverse_settlement` restored *outstanding* only, it did not reverse revenue) AND payment's
   **refund** (`reverse_payment` — `Dr A/R · Cr Cash`), so revenue, cash, and A/R **all net back to
   zero**. A refund-only cut would leave revenue overstated — the credit note is load-bearing. POS
   records an `is_return` ticket linked via `return_against`, flips the original → `returned`, emits
   `PosInvoiceReturned` (added to the union). **Idempotent:** both downstream reversals are idempotent
   and the ticket/event are gated on the original's `paid→returned` transition — a repeat return
   refunds/credits at most once. POS still posts no GL. Full-ticket returns only (partial parked).

## Consequences

- **Proven, not asserted:** `tests/retail_sale_seam.rs` runs POS → billing → payment → accounting: ring
  a 100,000 cash sale → recognise → billing posts `Dr A/R 100,000 · Cr Revenue 100,000`, payment posts
  `Dr Cash 100,000 · Cr A/R 100,000` → **A/R nets to zero**, cash holds 100,000, ticket `paid`, billing
  invoice `paid`; close reconciles the drawer (opening 500,000 + tender 100,000 = 600,000, counted
  balances).
- **Extension-contract §5 discharged:** `scripts/retail_sale_seam_roundtrip.sh` regenerates POS +
  billing + payment and asserts every port/ACL file is byte-identical and the seam stays green.
- This is the **first four-module seam** and the retail composition of the proven billing + payment
  emitters — POS adds a sales channel without a new GL leg.
- **Returns BUILT** (completeness council 2026-07-05, decision 5). Deferred (per the brief):
  consolidated billing handoff, offline sync, PPN-on-receipt, fiscal printer, multi-currency, over/short
  write-off automation, and **partial / line-level returns** (full-ticket is the correct MVP floor).
  Residual: a production bus + composition service to own the ports.
- **Parked with gates (completeness council):** (a) **consolidated handoff** — per-invoice is the
  stricter *complete* case (brief's own order: per-invoice first, then consolidated); gate = a merchant
  handoff exceeds N invoices/session. (b) **over/short write-off GL posting** — `difference_total` is
  computed + persisted + event-published; gate = an accounting consumer subscribes to the drawer-variance
  event. (c) **partial/line-level returns** — reuse `credit_note` with a line subset; gate = merchant
  demand. (d) the maturity-council parks (concurrency lock, `settle` crash-window) still stand.
- **Parked with gates (maturity council 2026-07-05):** (a) **per-ticket concurrency lock** — persist-
  and-reuse closes the *sequential* retry; two *concurrent* recognises on one ticket could still both
  raise. Gate: a POS terminal owns one ticket (single-flighted), so add a `pg_advisory_xact_lock` only
  if the production composition proves concurrent same-ticket recognition is reachable. (b) **`settle`
  idempotency** — a crash between `settle` committing and the `draft→paid` flip would double-settle on
  retry (reuse skips billing but re-runs settle). Gate: when a production payment adapter + crash-
  recovery story exists, persist the `payment_id` as a second skip-gate or give `SettlementRequest` a
  deterministic `idempotency_key = pos_invoice_id`. (c) **abandoned-draft drawer leak** — `close_session`
  excludes tenders on never-recognised tickets; surface them as a distinct "unrecognised" line (read-only,
  no `expected` change). (d) **billing UNIQUE on `source_pos_id`** — the durable belt-and-suspenders, but
  it lives in billing's schema (never touched from POS) — raise as a cross-module ticket.

## Addendum 2026-07-14 — refund contract made bus-satisfiable (maturity-council probe)

**Status**: Accepted — supersedes the `RefundRequest` shape in decision 5 and resolves maturity park (b).

The maturity council probed decision 2's "proven, not asserted" claim by forcing the seam onto a
topology where the four schemas do **not** co-locate. It found the refund path was only satisfiable in
one DB: `PaymentAdapter::refund` resolved the settling payment with a cross-schema
`SELECT payment_id FROM payment.payment_allocations WHERE invoice_ref=$1` — a read into payment's private
table. `RefundRequest` carried `{company_id, invoice_ref, amount}` with **no `payment_id`**, and POS
discarded the `payment_id` it received from `settle` (no column held it; the replay path fabricated
`Uuid::nil()`). Over a bus that query does not exist, so **returns — a BUILT feature — did not survive
the residual**. This was a hole in the *contract*, not merely unwired plumbing.

Fix (additive; pre-production ABI change taken now rather than after a production adapter freezes it):

1. **`RefundRequest` gains `payment_id`** (`pos_ports.rs`). The refund is now self-contained.
2. **POS persists the settling payment** on the ticket: new column `pos_invoice.payment_entry_id`
   (schema + migration `20260426220010`), written in the same `draft→paid` flip that stamps
   `billing_invoice_id`. `return_sale` reads it and hands it to `refund`; `short_circuit_paid` returns
   the real id on replay instead of `Uuid::nil()`.
3. **Resolves park (b)** — the persisted `payment_entry_id` is exactly the "second skip-gate" that park
   proposed, so a crash between `settle` and the flip no longer double-settles on retry.

The seam test's `PaymentAdapter` no longer holds a `PgPool` and does no cross-schema read, demonstrating
the refund would survive payment on its own database. Not yet done (still parked): splitting
`apply_settlement` out of `PaymentPort::settle` so settlement is not a two-service write with no
compensation — tracked separately as it needs a compensation story, not just a field.

The §5 byte-identical guard (`scripts/retail_sale_seam_roundtrip.sh`) is unaffected: every file it
snapshots is `user_owned` in `metaphor.codegen.yaml`, so regen skips them and they stay byte-identical
by construction — the guard proves regen-survival, not hand-edit-freeze.
