<!-- Reader: App developer · Mode: Tutorial → How-to -->
# Developer Guide

Ring a real counter sale — open a session, ring a ticket, take tender, close the drawer — in about
fifteen minutes, then wire the parts a production register needs. `backbone-pos` is a **built
module**, not a skeleton to stamp: you compose it into a `backend-service`, mount its router, and
call its write path. The tutorial part holds your hand once; the recipes assume you know your way
around.

Commands here were run against `metaphor 0.2.0`. Where the top-level [README](../../README.md) shows
a `backbone-schema`/`backbone` command, use the `metaphor` form below — those are the ones that work
today.

## Prerequisites

- **Rust** (2021 edition toolchain) and **Cargo**.
- The **`metaphor`** CLI on your `PATH` (`metaphor --version` → `metaphor 0.2.0` or newer).
- A reachable **PostgreSQL** instance. POS owns its own Postgres schema, `pos`.

## Quickstart — prove the toolchain end to end

Point at a database and exercise the shipped module. No renaming, no scaffolding — the entities are
already the POS domain (`PosProfile`, `PosOpeningEntry`, `PosInvoice`, `PosInvoiceItem`, `PosPayment`,
`PosClosingEntry`).

```bash
# From the module directory:
export DATABASE_URL="postgresql://root:password@localhost:5432/pos_dev"

# 1. Validate the schema YAML (the source of truth for every entity).
metaphor schema schema validate

# 2. Apply the migrations: creates the `pos` schema, the POS enums
#    (pos_session_status / pos_invoice_status / pos_payment_method / pos_closing_status)
#    and the six tables — pos_profiles, pos_opening_entries, pos_invoices,
#    pos_invoice_items, pos_payments, pos_closing_entries.
metaphor migration run

# 3. Run the module's tests.
metaphor dev test
```

What green looks like:

- **validate** passes with no schema errors.
- **migration run** reports the `pos` schema, the four enums, and the six tables created.
- **dev test** is green across the three suites: the **golden cases**
  (`tests/pos_golden_cases.rs` — ring totals, IDR rounding, multi-tender + change, close
  reconciliation, validation gates), the **integrity probes** (`tests/integrity_probes.rs` —
  not-fully-tendered, missing-account, recognition idempotency, decline-then-retry raises billing
  exactly once), and the **retail sale seam** (`tests/retail_sale_seam.rs` — POS ↔ billing ↔
  payment ↔ accounting, A/R nets to zero).

If the seam test skips, it needs a database on the port it expects (`DATABASE_URL` pointing at a live
Postgres); see [Troubleshooting](#troubleshooting).

## Compose and mount

`backbone-pos` is a library. A `backend-service` builds the module over a `PgPool` and mounts its
router. Use the **guarded** router — the recommended surface.

```rust
use backbone_pos::PosModule;
use backbone_pos::presentation::http::create_guarded_pos_routes;

let pos = PosModule::builder().with_database(pool.clone()).build()?;
let router = create_guarded_pos_routes(&pos, pool.clone());
// mount `router` in your Axum app, e.g. app.merge(router)
```

Two surfaces, and it matters which you mount:

- **`create_guarded_pos_routes(&pos, pool)`** — the recommended mount. It exposes **read documents**
  (profile / session / invoice reads) plus **validated writes** (open session, ring sale, add tender,
  close session). Generic CRUD mutation is **not** mounted, so a caller cannot POST a ticket with
  inconsistent totals or reopen a closed session.
- **`pos.all_crud_routes()`** — the full **unguarded** surface: the 12 generic CRUD endpoints on every
  entity. Use it only for trusted/admin tooling or seeding. (`pos.routes()` is a deprecated alias for
  it.)

Sale **recognition** and **returns** are not HTTP routes — they need the billing/payment ports, so
they are service/job-driven (see [Recipes](#recipes)).

## Walk a real counter sale (curl)

This is the marquee. Every request body below matches the field names in
[`guarded_routes.rs`](../../src/presentation/http/guarded_routes.rs) exactly — **bodies are
camelCase on the wire** even though the Rust and SQL are snake_case. Assume the service mounted the
guarded router at the root.

A `PosProfile` and its accounts must already exist (see
[How do I configure a register?](#how-do-i-configure-a-register)). Swap the UUIDs below for yours.

### 1. Open a session (the till)

```bash
curl -s -X POST localhost:8080/pos-sessions \
  -H 'content-type: application/json' \
  -d '{
    "companyId": "00000000-0000-0000-0000-0000000000c0",
    "posProfileId": "00000000-0000-0000-0000-0000000000f0",
    "cashierPartyId": "00000000-0000-0000-0000-0000000000ca",
    "openedAt": "2026-07-05T08:00:00",
    "openingBalances": [ { "method": "cash", "amount": "500000" } ]
  }'
# → 201 { "id": "…" }   ← this id is the openingEntryId; keep it
```

### 2. Ring the ticket

A single line: 1 unit @ 100,000, no discount, no PPN, receipt rounded to the nearest 100.

```bash
curl -s -X POST localhost:8080/pos-sales \
  -H 'content-type: application/json' \
  -d '{
    "companyId": "00000000-0000-0000-0000-0000000000c0",
    "posProfileId": "00000000-0000-0000-0000-0000000000f0",
    "openingEntryId": "<opening id from step 1>",
    "receiptNumber": "R-0001",
    "postingAt": "2026-07-05T08:05:00",
    "lines": [
      { "itemId": "00000000-0000-0000-0000-000000000001",
        "quantity": "1", "unitPrice": "100000", "discountAmount": "0" }
    ],
    "taxTotal": "0",
    "roundTo": "100"
  }'
# → 201 { "id": "…" }   ← the ticket (posInvoiceId); it starts `draft`
```

Money is computed **server-side**: `net = money(qty·price) − discount`, `grand = net + tax`,
`rounded = round_to(grand, step)`, `rounding_adjustment = rounded − grand`. Here net = grand =
rounded = 100,000, adjustment 0.

### 3. Take tender — multi-tender with change

Ring it up as the PGC-3 golden case: 60,000 on card, then 50,000 cash, on a 100,000 ticket → the
cash overpayment becomes change.

```bash
# Tender 1 — card 60,000
curl -s -X POST localhost:8080/pos-tenders \
  -H 'content-type: application/json' \
  -d '{
    "posInvoiceId": "<ticket id from step 2>",
    "paymentMethod": "card",
    "amount": "60000",
    "referenceNo": "APPROVAL-778"
  }'
# → 200 { "paidTotal": "60000", "changeDue": "0", "fullyTendered": false }

# Tender 2 — cash 50,000 (110,000 tendered on a 100,000 ticket)
curl -s -X POST localhost:8080/pos-tenders \
  -H 'content-type: application/json' \
  -d '{
    "posInvoiceId": "<ticket id from step 2>",
    "paymentMethod": "cash",
    "amount": "50000"
  }'
# → 200 { "paidTotal": "110000", "changeDue": "10000", "fullyTendered": true }
```

`paid_total = Σ tenders`; `change_due = max(0, paid − rounded)`; `fully_tendered = paid ≥ rounded`.
A ticket must be `draft` to accept a tender (`not_draft` otherwise). `referenceNo` is optional.

### 4. Recognise the sale (service/job — not HTTP)

Once fully tendered, recognition drives billing (raise + post the real Sales Invoice → revenue) then
payment (settle the tender → cash), so retail reuses the same GL emitters as web/B2B sales. Because
it needs the `BillingPort`/`PaymentPort`, **it is not an HTTP route** — call it from a service or
job in your composition layer:

```rust
// billing + payment are your adapters implementing BillingPort / PaymentPort
let outcome = pos_write.recognize_sale(ticket_id, &billing, &payment).await?;
// outcome.billing_invoice_id, outcome.payment_id
```

For a 100,000 cash sale this books `Dr A/R · Cr Revenue` (billing) then `Dr Cash · Cr A/R`
(payment): **A/R nets to zero at the counter**, cash holds the takings, and the ticket flips
`draft → paid` with `PosInvoicePaid` emitted once. Recognition is **idempotent** (short-circuits on
`paid`) and **at-most-once on billing** (a decline-then-retry reuses the raised invoice — no double
revenue). See [`tests/retail_sale_seam.rs`](../../tests/retail_sale_seam.rs) for a working
composition of the two ports over the real billing/payment services.

### 5. Close the drawer (Z-report)

```bash
curl -s -X POST localhost:8080/pos-sessions/close \
  -H 'content-type: application/json' \
  -d '{
    "companyId": "00000000-0000-0000-0000-0000000000c0",
    "openingEntryId": "<opening id from step 1>",
    "cashierPartyId": "00000000-0000-0000-0000-0000000000ca",
    "closedAt": "2026-07-05T20:00:00",
    "counted": [ { "method": "cash", "amount": "540000" }, { "method": "card", "amount": "60000" } ]
  }'
# → 200 { "closingId": "…", "differenceTotal": "0" }
```

Per method, `expected = opening_float + Σ recognised tenders` (cash also `− Σ change_due`);
`difference = counted − expected`; `differenceTotal = Σ`. With opening cash 500,000, a recognised
50,000 cash tender less 10,000 change (net +40,000) and a 60,000 card tender, the drawer expects
540,000 cash and 60,000 card — count those and the difference is zero. The session flips to
`closed`; a further sale against it fails `session_not_open`.

> Returns follow the same service-driven shape as recognition:
> `pos_write.return_sale(ticket_id, &billing, &payment).await?` reverses **both** legs
> (`payment.refund` + `billing.credit_note`) so revenue, cash, and A/R all net back to zero. See the
> [returns recipe](#how-do-i-handle-a-return).

## Key concepts

Eight ideas carry you the rest of the way. One line each; the linked page explains *why*.

- **Schema YAML is the source of truth.** You edit [`schema/models/*.model.yaml`](../../schema/models);
  the entities, DTOs, migrations, repositories, services, handlers, and routes are generated from it.
  ([Philosophy](01-philosophy.md).)
- **POS posts no GL.** On recognition it *orchestrates* `backbone-billing` (revenue) and
  `backbone-payment` (settlement) through outbound ports — "orchestrate, don't re-implement."
  ([Architecture](04-architecture.md).)
- **Money is server-side; IDR receipts round.** `ring_sale` computes net/grand/rounded and the
  rounding adjustment; `roundTo` sets the nearest-step receipt rounding (e.g. 100).
- **The guarded surface vs unguarded CRUD.** `create_guarded_pos_routes` = read documents + validated
  writes; `all_crud_routes()` is the full unguarded surface (trusted/admin/seeding only).
- **Multi-tender + change.** A ticket takes many tender lines; `paid_total` and `change_due` recompute
  on each; `fully_tendered` gates recognition.
- **Recognition is idempotent and at-most-once on billing.** Re-calling `recognize_sale` short-circuits
  on `paid`; a decline-then-retry reuses the raised invoice so revenue is never doubled.
- **Close = drawer reconciliation (Z-report).** Expected-vs-counted per tender method; the difference
  total is the over/short.
- **Custom code survives regeneration.** The hand-authored POS write path, ports, events, and guarded
  routes are `user_owned`; generated code is safe only inside `// <<< CUSTOM` markers.
  ([ADR-0003](adr/adr-0003-custom-markers.md).)

## Recipes

### How do I recognise a sale?

Recognition needs a `BillingPort` and a `PaymentPort`. Implement each over your real
`backbone-billing` / `backbone-payment` services in your composition layer (the shipped POS library
has no Cargo edge to either — that is deliberate), then call `recognize_sale`:

```rust
use backbone_pos::application::service::pos_ports::{BillingPort, PaymentPort};

struct MyBilling { /* holds billing service */ }
#[async_trait::async_trait]
impl BillingPort for MyBilling {
    async fn raise_and_post(&self, req: &SaleInvoiceRequest) -> Result<InvoiceAck, PosRejected> { /* … */ }
    async fn credit_note(&self, req: &CreditNoteRequest) -> Result<ReversalAck, PosRejected> { /* … */ }
}
// …and MyPayment: PaymentPort { settle, refund }

let outcome = pos_write.recognize_sale(ticket_id, &billing, &payment).await?;
```

The ports are the wire contract — `SaleInvoiceRequest → InvoiceAck`, `SettlementRequest →
SettlementAck`, and `PosRejected { code, message }` for failures. A full working composition (POS ↔
billing ↔ payment ↔ accounting) lives in [`tests/retail_sale_seam.rs`](../../tests/retail_sale_seam.rs).

### How do I subscribe to POS events?

Implement `PosEventSink` and pass it via `PosWriteService::with_sink`. The default sink
(`LoggingSink`) just traces; a real one wires your bus. `PosInvoicePaid` carries the billing invoice
and payment, so a loyalty/analytics consumer needn't call back into POS.

```rust
use std::sync::Arc;
use backbone_pos::application::service::pos_events::{PosEvent, PosEventSink};
use backbone_pos::application::service::pos_write_service::PosWriteService;

struct LoyaltySink;
impl PosEventSink for LoyaltySink {
    fn publish(&self, event: PosEvent) {
        if let PosEvent::PosInvoicePaid(p) = event {
            // award points, push to analytics, … (p.billing_invoice_id, p.payment_id, p.rounded_total)
        }
    }
}

let pos_write = PosWriteService::with_sink(pool.clone(), Arc::new(LoyaltySink));
```

The union also carries `PosSessionOpened`, `PosSessionClosed` (with `difference_total`), and
`PosInvoiceReturned`.

### How do I add a tender method?

Tender methods are the `PosPaymentMethod` enum in the schema — today: `cash`, `card`, `qris`,
`e_wallet`, `bank_transfer`, `virtual_account`. On the wire, `paymentMethod` is the string form (e.g.
`"qris"`). To add one, edit the enum in the schema YAML, regenerate, and generate a migration for the
new enum value — never hand-edit the generated Rust. See the
[Maintainer Guide](05-maintainer-guide.md).

### How do I configure a register?

A register is a `PosProfile` row. It carries the currency (default IDR), `allow_discount`,
`is_active`, and the **GL account refs the recognition handoff needs**: `income_account_id`
(revenue), `receivable_account_id` (A/R control), `cash_account_id` (where tender lands),
`write_off_account_id` (over/short at close), and `default_customer_id` (walk-in). Those account ids
are logical FKs into `accounting.Account` — no DB constraint, but recognition fails
`missing_account` if `receivable_account_id`, `cash_account_id`, or a revenue account is absent.
Create the profile via the unguarded CRUD surface (`all_crud_routes()`) or a seeder.

### How do I handle a return?

Returns are service-driven, like recognition. Call `return_sale` on a **recognised** (`paid`) ticket:

```rust
let outcome = pos_write.return_sale(original_ticket_id, &billing, &payment).await?;
```

It reverses **both** legs — `payment.refund` (`Dr A/R · Cr Cash`) and `billing.credit_note`
(`Dr Revenue · Cr A/R`, invoice → cancelled) — so revenue, cash, and A/R all net back to zero. It
records an `is_return` ticket linked via `return_against`, flips the original → `returned`, and emits
`PosInvoiceReturned`. It is idempotent (a repeat refunds/credits at most once) and returns
`not_returnable` if the ticket was never recognised. Returns are full-ticket only today (line-level
returns are a stated non-goal).

## Configuration

Defaults live in [`config/application.yml`](../../config/application.yml); `application-dev.yml` and
`application-prod.yml` layer over it, and `DATABASE_URL` in the environment always wins.

| Option | Default | When to change |
|--------|---------|----------------|
| `server.host` | `0.0.0.0` | Bind to a specific interface. |
| `server.port` | `8080` | Port conflicts / multi-service hosts. |
| `server.grpc_port` | `50051` | Only if you enable a gRPC surface (proto/gRPC generators are disabled here). |
| `database.url` | `postgresql://root:password@localhost:5432/skeletondb` | **Always** in real deployments — override with the `DATABASE_URL` env var, which takes precedence. |
| `database.max_connections` | `10` | Tune to your Postgres pool budget. |
| `database.min_connections` | `5` | Warm-pool floor. |
| `logging.level` | `info` | `debug`/`trace` when diagnosing; `warn` in noisy prod. |
| `logging.format` | `json` | `pretty` locally (the dev overlay sets this). |
| `module.name` | `pos` | The module / Postgres schema name; leave it. |
| `features.soft_delete` | `true` | Rows carry `deleted_at`; the write path already filters it. |
| `entities.<name>.cache_ttl` | `300` | Per-entity read cache seconds. |
| `entities.<name>.pagination.default_limit` / `max_limit` | `20` / `100` | List page sizes on the read routes. |
| `tracing.enabled` / `metrics.enabled` | `true` | Off in the dev overlay; wire your OTLP endpoint in prod. |

## Troubleshooting

Write-path errors surface as `{ "error": "<code>", "message": "…" }` with HTTP 422 for validation,
404 for not-found. The codes come straight from
[`pos_write_service.rs`](../../src/application/service/pos_write_service.rs).

| Symptom (`error` code) | Cause | Fix |
|------------------------|-------|-----|
| `empty_document` | Ringing a sale with no `lines` | Send at least one line. |
| `negative_amount` | A negative quantity, price, discount, or tender amount | All money must be ≥ 0 (tender > 0). |
| `session_not_open` | Ringing against a closed/unknown session | Open a session first; don't ring against a `closed` `openingEntryId`. |
| `not_draft` | Tendering a ticket that is already `paid`/`void`/`returned` | Tenders apply only to a `draft` ticket. |
| `not_fully_tendered` | `recognize_sale` before `paid_total ≥ rounded_total` | Take more tender until `fullyTendered: true`. |
| `missing_account` | Recognition with no `receivable_account_id` / `cash_account_id` / revenue account (or PPN with no tax account) | Configure the `PosProfile` accounts; PPN needs a tax account before it is accepted. |
| `duplicate_number` | A `receiptNumber` already used | Receipt numbers are unique per company; use a fresh one. |
| `not_returnable` | `return_sale` on a ticket that was never recognised | Only a `paid` (or already-`returned`) ticket can be returned. |
| `backbone-schema: command not found` | Following an older framework doc/skill | There is no standalone `backbone-schema`/`backbone` binary — use `metaphor schema schema …`. |
| JSON field rejected / null (`itemId` vs `item_id`) | Sending snake_case on the wire | Request bodies are **camelCase** by design; snake_case is DB/Rust only. |

---

Next: [Contributing](07-contributing.md) to send a change back, or the
[Glossary](08-glossary.md) to pin down a term.
