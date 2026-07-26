# backbone-pos — End-to-End Use Cases & Data Transformation

> **One reader, one mode:** App developers and integrators who need to understand what POS does,
> how a sale flows end-to-end, how the data transforms into the ledger, and how to compose POS into
> a service. *Diátaxis: Guide + Reference.*
>
> This doc consolidates the **end-to-end picture**. For depth, link out, don't duplicate:
> the [Handbook](README.md) (architecture, developer guide, glossary), [Business flows → golden cases](business-flows/golden-cases.md)
> (the numeric oracle), and [ADR-001](adr/ADR-001-pos-boundary-and-retail-seam.md) (the boundary decision).

---

## 1. What POS is — and isn't

`backbone-pos` is the **retail counter path** (the in-store cashier) of an Indonesia-first ERP's
Financials pillar. It owns four things and **only** four things:

1. the **cashier session** (open / X-report / close, till float, cash movements),
2. the **sale ticket** (line items, money math, IDR receipt rounding),
3. the **tender** (multi-tender, change, fully-tendered detection), and
4. the **drawer** (Z-report reconciliation: expected vs counted cash).

**POS posts no General Ledger itself.** On sale recognition it *orchestrates* the two existing GL
emitters — `backbone-billing` (revenue) and `backbone-payment` (settlement), optionally
`backbone-inventory` (stock issue) — through **outbound ports** (traits). The ledger contract stays
singular: a cash sale at the counter produces the **same** `Dr A/R · Cr Revenue` then
`Dr Cash · Cr A/R` posts as a web/B2B sale, so **A/R nets to zero at the counter**.

### POS is retail, not ecommerce

POS is **domain-specific to in-store checkout** (cashier, till, drawer, physical tenders like
cash / card / QRIS / e-wallet). It is **not** an ecommerce/online-cart module. Online channels live
in peer modules (`backbone-selling`, `backbone-orders`). POS and ecommerce are **sibling sales
channels that both feed the same `backbone-billing` + `backbone-payment` + `backbone-accounting`** —
POS adds a retail channel without inventing a new GL leg. If you are building online checkout, you do
not consume POS; you consume billing/payment directly (or via the selling/orders modules).

### Why `backbone-catalog` is **not** a POS dependency (by design)

`backbone-catalog` owns canonical **Item identity** (SKU, barcode, name, UOM, variants) — *no prices,
no stock, no tax rules*. POS does **not** depend on it. Instead:

- POS stores `PosInvoiceItem.item_id` as a **logical FK** to `catalog.Item.id` (raw `uuid`, no
  relation loading, no compile-time edge).
- POS has **no `CatalogPort` / `ProductPort`** — it never looks products up. The **composing app**
  resolves items and prices (via catalog + promo) and *passes* `item_id`, `description`,
  `unit_price`, and `revenue_account_id` into POS on each sale line.
- This is the same logical-FK + projection discipline POS uses downstream (ports) — catalog is just
  **upstream** instead of downstream. POS stays independently deployable.

---

## 2. The shape of the module

### Entities (schema YAML is the source of truth — `schema/models/`)

| Entity | Role | Key fields |
|---|---|---|
| **PosProfile** | Register config + the GL accounts the handoff needs | `company_id`, `branch_id`, `warehouse_id`, `income_account_id`, `receivable_account_id`, `cash_account_id`, `tax_account_id`, `cogs_account_id`, `inventory_account_id`, `tax_rate`, `currency`, `default_customer_id` |
| **PosOpeningEntry** | Cashier session (till open) | `pos_profile_id`, `cashier_party_id`, `opening_balances` (JSON per method), `status: open→closed` |
| **PosInvoice** | The sale ticket | `receipt_number`, money totals (below), `billing_invoice_id`, `payment_entry_id`, `is_return`, `return_against`, `status: draft→paid→returned` |
| **PosInvoiceItem** *(child)* | Sale line | `item_id` (logical FK → catalog), `description`, `quantity`, `unit_price`, `discount_amount`, `net_amount`, `revenue_account_id` |
| **PosPayment** *(child)* | Tender line | `payment_method` (cash/card/qris/e_wallet/bank_transfer/virtual_account), `amount`, `reference_no`, `payment_entry_id` |
| **PosClosingEntry** | Z-report | `totals_by_method` (JSON), `grand_total`, `invoice_count`, `difference_total`, `status: draft→submitted→reconciled` |
| **PosCashMovement** | Non-sale drawer cash | `movement_type` (pay_in/pay_out/drop/no_sale), `amount`, `reason` |

### Outbound ports (the wire contract — `src/application/service/pos_ports.rs`)

POS depends on **no** domain module at compile time (zero normal Cargo edges; the siblings are
dev-deps for the seam test only). It talks downstream through traits:

```rust
trait BillingPort   { async fn raise_and_post(&self, req: &SaleInvoiceRequest) -> Result<InvoiceAck, PosRejected>;
                      async fn credit_note(&self, req: &CreditNoteRequest)  -> Result<ReversalAck, PosRejected>; }
trait PaymentPort   { async fn settle(&self, req: &SettlementRequest)       -> Result<SettlementAck, PosRejected>;
                      async fn refund(&self, req: &RefundRequest)           -> Result<ReversalAck, PosRejected>; }
trait InventoryPort { async fn issue(&self, req: &StockIssueRequest)        -> Result<StockIssueAck, PosRejected>; }
trait CartPricingPort { /* server-authoritative promo pricing for a cart */ }   // src/application/service/pos_cart_pricing.rs
trait PosEventSink    { async fn publish(&self, evt: PosEvent); }             // src/application/service/pos_events.rs
```

The **composing service** implements these over the real modules (see §5).

### Events (`src/application/service/pos_events.rs`)

`PosEvent` union: `PosSessionOpened`, **`PosTenderCompleted`** (the recognition trigger),
`PosInvoicePaid`, `PosInvoiceReturned`, `PosSessionClosed`. When an outbox schema is configured,
`PosTenderCompleted` is **staged inside the tender transaction** (`pos_tender.rs`) so recognition
survives a crash between commit and the in-process sink — drained by an outbox relay.

---

## 3. End-to-end use cases

All write paths are **guarded** (`src/presentation/http/guarded_routes.rs`): tenant-scoped via a JWT
`CompanyContext`, with validated, domain-aware handlers — **no generic CRUD** on the production
surface (`PosModule::all_crud_routes()` exists but is admin/trusted-only).

### UC-1 — Cashier counter sale (cash)  · *the canonical flow*

```
POST /pos-sessions            open session (opening float per method)      → PosOpeningEntry (open)
POST /pos-sales               ring the cart                                 → PosInvoice (draft) + PosInvoiceItem[]
POST /pos-tenders             add a cash tender ≥ rounded_total            → PosPayment; emits PosTenderCompleted
                             ─── recognize_sale (driven by the event) ───
                            BillingPort.raise_and_post  → billing SalesInvoice : Dr A/R · Cr Revenue
                             link billing_invoice_id (ticket still draft — at-most-once)
                             PaymentPort.settle         → payment PaymentEntry : Dr Cash · Cr A/R
                             flip draft→paid, persist payment_entry_id, emit PosInvoicePaid
                             (optional) InventoryPort.issue → stock decrement + COGS post
POST /pos-sessions/close      Z-report: expected vs counted cash            → PosClosingEntry
```

**Net ledger for a 100,000 cash sale:** revenue `−100,000` (Cr), cash `+100,000` (Dr), **A/R `0`**
(the Cr A/R from billing and the Dr A/R from payment cancel). POS wrote none of these journals — it
drove the emitters that did.

### UC-2 — Multi-tender sale with change

Ring a 100,000 sale; tender 60,000 card then 50,000 cash. After the card tender the ticket is **not
fully tendered** (no recognition). The cash tender crosses the threshold → `paid_total = 110,000`,
`change_due = 10,000`, `PosTenderCompleted` fires once. Recognition settles `amount = rounded_total`
(100,000) — change is a POS/UI concern, not a settlement amount.

### UC-3 — Promo-priced cart sale (the priced route)

`POST /pos-sales` has a **priced** variant (`create_guarded_pos_priced_route_with_outbox`) that calls
`CartPricingPort` for **server-authoritative** discounts before computing line nets. The composing
service implements `CartPricingPort` over its promo engine (e.g. `backbone-promo`). The cart carries
`item_id`, `item_group_id`, `brand_id` so the promo engine can match rules; POS still does all money
math server-side after pricing.

### UC-4 — Return / refund a paid sale

`return_sale` drives **both** downstream legs in reverse so revenue, cash, and A/R all return to
zero:

- `BillingPort.credit_note` → `Dr Revenue · Cr A/R` (billing invoice → cancelled),
- `PaymentPort.refund` → `Dr A/R · Cr Cash` (uses the persisted `payment_entry_id` so the refund is
  self-contained and bus-satisfiable).

POS records an `is_return` ticket linked via `return_against`, flips the original `paid → returned`,
emits `PosInvoiceReturned`. Both reversals are idempotent and gated on the `paid→returned`
transition — a repeat return refunds/credits **at most once**. (Full-ticket returns only; partial is
parked.)

### UC-5 — Mid-shift drawer management

- `POST /pos-cash-movements` — pay-in / pay-out / drop / no-sale (non-sale cash, tracked for
  reconciliation).
- `GET /pos-sessions/:id/x-report` — mid-shift drawer read (expected cash so far).
- `POST /pos-sessions/close` — Z-report: per-method `expected = opening_float + Σ recognised tenders
  (cash − change)` vs `counted` → `difference_total` (over/short).

### UC-6 — Custom integration: composing POS into your own service

POS is a **library**, not a server. To use it you compose it (this is exactly what
`serpa-posman-service` does — see §5):

1. `PosModule::builder().with_database(pool).build()?`
2. **Implement the ports** over your real services — `BillingPort` over billing's Sales-Invoice
   create+post, `PaymentPort` over payment's create+post+settlement, `InventoryPort` over inventory's
   stock issue, `CartPricingPort` over your promo engine.
3. **Implement `PosEventSink`** so that on `PosTenderCompleted` it calls
   `PosWriteService::recognize_sale(invoice_id, &billing, &payment, inventory)`.
4. **Mount the guarded routes** (`create_guarded_pos_routes_with_outbox`) with a `TenantVerifier`.
5. **Run an outbox relay** draining `pos.outbox_events` → recognition as a durable backstop.

**Custom-channel example (self-checkout kiosk):** the *same* flow as UC-1 with
`cashier_party_id` set to the kiosk/system principal; tenders are card/QRIS only (no change, no cash
drawer). No POS code changes — only the composing app's port adapters and tender mix differ.

---

## 4. How the data transforms (ticket → invoice → settlement → GL)

### 4.1 Money math (server-side, at ring — `pos_sale.rs`)

```
line.net_amount     = money(quantity · unit_price) − discount_amount
net_total           = Σ line.net_amount
tax_total           = net_total · profile.tax_rate          # server-side PPN; refused until a tax account exists
grand_total         = net_total + tax_total
rounding_adjustment = round_to(grand_total, step) − grand_total   # IDR, nearest 100
rounded_total       = round_to(grand_total, step)
paid_total          = Σ tender.amount
change_due          = max(0, paid_total − rounded_total)     # cash overpayment
```

Money is exact IDR (`rust_decimal`, 2 dp, half-away-from-zero). Recognition requires
`paid_total ≥ rounded_total` (fully tendered).

### 4.2 Field mapping on recognition (`pos_recognition.rs`)

| POS source | → `SaleInvoiceRequest` (billing) | → `SettlementRequest` (payment) |
|---|---|---|
| `PosInvoice.id` | `source_pos_id` | — |
| raised billing invoice id | — | `invoice_ref` |
| `PosInvoice.customer_id` (or profile default) | `customer_id` | `customer_id` |
| `profile.receivable_account_id` | `receivable_account_id` | `party_account_id` |
| `profile.cash_account_id` | — | `bank_account_id` |
| each `PosInvoiceItem` | `SaleLine { item_id, revenue_account_id, quantity = 1, unit_price = net_amount }` | — |
| `PosInvoice.tax_total / tax_account_id / tax_rate` | same | — |
| `PosInvoice.rounded_total` | — | `amount` |
| `PosInvoice.posting_at` / `company_id` / `currency` | same | same |

> **Collapse to net:** POS sends each line as `quantity = 1, unit_price = net_amount` — the
> `qty·price − discount` math is resolved *before* the handoff, so billing books net revenue per line.
> `item_id` is forwarded as-is (the logical catalog FK) — billing does not look it up either.

If the profile has a warehouse + COGS/inventory accounts, recognition also calls
`InventoryPort.issue` with `StockIssueLine { item_id, quantity }` per sold item (the *real* quantity,
for stock decrement) plus the profile's warehouse/account dimensions.

### 4.3 The resulting ledger (cash sale)

| Post | Dr | Cr |
|---|---|---|
| **billing** raise+post Sales Invoice | A/R `rounded_total` | Revenue `Σ net` + Tax `tax_total` |
| **payment** settle | Cash `rounded_total` | A/R `rounded_total` |
| **net** | Cash `rounded_total` | Revenue `net_total` + Tax `tax_total` · **A/R = 0** |

### 4.4 The recognition state machine + saga guarantees

```
draft ──recognize_sale──▶ paid ──return_sale──▶ returned
        (at-most-once)          (idempotent)
```

- **Billing is raised at most once.** `raise_and_post` is a saga step (billing commits revenue
  *before* payment settles). The instant it returns, POS persists `billing_invoice_id` on the still-
  `draft` ticket; any re-entry (e.g. card declined then retried) **reuses** that invoice and skips the
  raise — otherwise a retry would book a second revenue journal for one sale (proven by integrity
  probe IP-4).
- **Settlement has a skip-gate too.** POS persists `payment_entry_id` in the same `draft→paid` flip;
  `return_sale` reads it so the refund is self-contained, and a crash between `settle` and the flip no
  longer double-settles on retry.
- `recognize_sale` short-circuits on `status = paid` (the link/event fire only on the `draft→paid`
  transition → once-only event).

---

## 5. How POS is consumed today

**One production consumer:** **`serpa-posman-service`**
(`products/serpa-workspace/apps/serpa-posman-service`). It is the composition service that owns the
port implementations and the durable bus:

| POS port | serpa-posman adapter | backs onto |
|---|---|---|
| `BillingPort` | `BillingAdapter` | `backbone_billing::BillingWriteService` (+ `GlAdapter` → accounting `PostingService`) |
| `PaymentPort` | `PaymentAdapter` | `backbone_payment::PaymentWriteService` (+ billing draw-down) |
| `InventoryPort` | `InventoryAdapter` | `backbone_inventory::InventoryWriteService` |
| `CartPricingPort` | `PromoCartPricer` | `backbone_promo::PromoWriteService` |
| `PosEventSink` | `RecognitionSink` | on `PosTenderCompleted` → `recognize_sale` |

It mounts `create_guarded_pos_routes_with_outbox` + the priced route, and runs an **outbox relay**
(`outbox_relay.rs`, schema `pos`) draining `pos.outbox_events → recognition` as the durable backstop
behind the in-process sink. The ports live **in POS** (interfaces); the adapters live **in the
consumer** (implementations) — that is what keeps POS free of normal Cargo edges to the other modules.

**Test-only consumer:** `backbone-promo` dev-depends on POS for the `cart_pos_seam` and
`price_resolution_seam` tests (not shipped).

> ⚠️ `serpa-posman-service` currently pins `backbone-pos = "v0.1.5"` — predating the dual-mode dep
> rewire / outbox route (POS is now `v0.2.1`). Bump the consumer's pin when adopting the current POS.

---

## 6. HTTP surface (guarded) — quick reference

| Method & path | Handler | Effect |
|---|---|---|
| `POST /pos-sessions` | `open_session` | open cashier session + opening float |
| `POST /pos-sales` | `ring_sale` | ring cart → draft ticket (+ priced variant via `CartPricingPort`) |
| `POST /pos-tenders` | `add_tender` | add tender; emits `PosTenderCompleted` when fully tendered |
| `POST /pos-cash-movements` | `record_cash_movement` | pay-in / pay-out / drop / no-sale |
| `GET  /pos-sessions/:id/x-report` | `x_report` | mid-shift drawer read |
| `GET  /pos-receipts/:id` | `receipt` | formatted receipt |
| `POST /pos-sessions/close` | `close_session` | Z-report reconciliation |
| generated list/get per entity | CRUD read | wrapped in `company_auth` |

All writes derive `company_id` from the authenticated JWT principal (never the request body), so
every row is tenant-scoped at the source.

---

## 7. FAQ

- **Is POS used for ecommerce?** No. POS is the in-store/cashier channel. Ecommerce/online checkout
  is a peer channel (`backbone-selling` / `backbone-orders`); both feed the same billing + payment +
  accounting. POS = physical retail only.
- **Why no `backbone-catalog` dependency?** Intentional. POS stores `item_id` as a logical FK to
  `catalog.Item.id`; the composing app resolves items/prices and passes them in. POS never looks
  products up, so it needs no catalog edge.
- **Does POS write to the GL?** No — it drives billing + payment (and optionally inventory) through
  ports. A cash sale nets A/R to zero *at the counter* using the same posts as any other sale.
- **What if a tender fails mid-recognition?** Safe. Billing is raised at most once (persist-and-reuse
  of `billing_invoice_id`); payment has a `payment_entry_id` skip-gate. Decline-then-retry raises
  revenue exactly once (IP-4).
- **Multi-currency?** IDR only today; multi-currency and PPN-on-receipt are parked behind gates.

---

*Keep this doc in step with [`business-flows/golden-cases.md`](business-flows/golden-cases.md) (the
numeric oracle) and [`ADR-001`](adr/ADR-001-pos-boundary-and-retail-seam.md) (the boundary). When code
and doc disagree, the schema YAML / code wins — the doc is the bug.*
