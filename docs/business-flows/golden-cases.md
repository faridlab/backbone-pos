# POS — Golden Cases (the numeric oracle)

Mirrors `tests/pos_golden_cases.rs`, `tests/integrity_probes.rs`, and the cross-module retail-sale seam
in `tests/retail_sale_seam.rs`. Money is exact IDR (2dp, half-away-from-zero).

## Write path (`tests/pos_golden_cases.rs`)

| Case | Input | Expected |
|------|-------|----------|
| **PGC-1** | ring: 2 × 50,000 − 5,000 discount, no tax | net `95,000`, grand `95,000`, rounded `95,000`; `draft`. |
| **PGC-2** | rounding to nearest 100 | 95,040 → rounded `95,000` (adj `−40`); 95,060 → rounded `95,100` (adj `+40`). |
| **PGC-3** | rounded 100,000; 60,000 card + 50,000 cash | after card: not fully tendered; after cash: fully tendered, paid `110,000`, change `10,000`. |
| **PGC-4** | close: opening cash 500,000, no sales, count 500,000 | difference `0`, expected cash `500,000`; session `closed`; a sale can no longer ring (`session_not_open`). |
| **PGC-5** | empty / negative / duplicate receipt | `empty_document` / `negative_amount` / `duplicate_number`. |

## Integrity probes (`tests/integrity_probes.rs`)

| Case | Input | Expected |
|------|-------|----------|
| **IP-1** | recognise a partially-tendered sale | `not_fully_tendered`; billing is **not** driven. |
| **IP-2** | recognise with a profile missing register accounts | `missing_account` (fails before any post). |
| **IP-3** | recognise the same sale twice | second short-circuits on `paid`; billing driven **once**, `PosInvoicePaid` emitted **once** (no double invoice/settlement/takings). |
| **IP-4** (council 2026-07-05) | tender fails (declined) then retries succeeds | after the decline: ticket `draft` but the Sales Invoice is raised + `billing_invoice_id` persisted; retry **reuses** it → billing raised **exactly once** (no double revenue), then `paid`. Fails against an always-raise (billing driven twice). |

## Retail-sale seam — POS ↔ billing ↔ payment ↔ accounting (`tests/retail_sale_seam.rs` + `scripts/retail_sale_seam_roundtrip.sh`)

| Case | Input | Expected |
|------|-------|----------|
| **RSSEAM-1** | ring a 100,000 cash sale + 100,000 tender → `recognize_sale` (drives billing + payment) → close | billing posts `Dr A/R 100,000 · Cr Revenue 100,000`; payment posts `Dr Cash 100,000 · Cr A/R 100,000`; **A/R nets to `0`**, revenue `−100,000` (credit), cash `100,000`; ticket `paid` + linked, billing invoice `paid`, `PosInvoicePaid` emitted. Close: expected cash `600,000` (opening 500,000 + tender), difference `0`. Zero normal Cargo edge. |
| **RSSEAM-2** (council 2026-07-05) | recognise a 100,000 sale, then `return_sale` | POS drives billing credit note (`Dr Revenue · Cr A/R`) + payment refund (`Dr A/R · Cr Cash`) → revenue `0`, cash `0`, **A/R `0`** (both legs reversed); original → `returned`, billing invoice → `cancelled`, an `is_return` ticket linked via `return_against`, `PosInvoiceReturned` emitted. Return again → **no double refund/credit**, one return ticket. Fails refund-only (revenue left at 100,000). |
| **§5 round-trip** | regen POS + billing + payment, re-run | all seam port/ACL files byte-identical; RSSEAM-1/2 still green. |

## Conventions
- POS posts **no GL** — it drives billing (revenue) + payment (settlement) via the ports; a cash sale
  nets A/R to zero at the counter.
- Recognition requires a fully-tendered ticket + register accounts; it is idempotent on `paid`.
- IDR receipt rounding to the configured step; PPN delegated to billing/tax (tax-free base for now).
- Drawer reconciliation: `expected = opening_float + Σ recognised tenders` (cash − change) vs counted.
