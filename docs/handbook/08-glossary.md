<!-- Reader: All · Mode: Reference -->
# Glossary — ubiquitous language

One term, one meaning, used everywhere in this handbook and in the code. When a term here names a
type or file, that name is exact. If you find a doc using a different word for one of these, the doc
is the bug.

### Aggregate / Entity
A domain object with identity and a lifecycle, defined by one `schema/models/<name>.model.yaml`.
In POS: `PosProfile`, `PosOpeningEntry`, `PosInvoice`, `PosInvoiceItem`, `PosPayment`,
`PosClosingEntry`. Generated into `src/domain/entity/<name>.rs` with a strongly-typed id
(`PosInvoiceId`), a builder, `apply_patch`, and audit accessors.

### Application layer
The use-case layer (`src/application/`): services and DTOs. Depends on the domain; knows nothing
about HTTP or SQL.

### Audit metadata
The `metadata` JSONB field (`created_at`, `updated_at`, `deleted_at`, `created_by`, `updated_by`,
`deleted_by`) added when `config.audit: true`. Timestamps are set by a Postgres trigger; the `*_by`
actor fields are logical FKs to `sapiens.User.id`.

### `BackboneCrudHandler`
The `backbone-core` type that produces an Axum `Router` with all **twelve** CRUD endpoints for an
entity. Invoked as `BackboneCrudHandler::<…>::routes(service, "/collection")`. You never hand-write
these routes.

### BillingPort / PaymentPort
The two **outbound orchestration ports** (`src/application/service/pos_ports.rs`): hand-authored
traits POS calls to drive revenue and settlement. `BillingPort { raise_and_post, credit_note }`,
`PaymentPort { settle, refund }`. They speak serde request/ack envelopes (`SaleInvoiceRequest →
InvoiceAck`, `SettlementRequest → SettlementAck`, `RefundRequest → ReversalAck`, `PosRejected`), so
the shipped POS library has **zero normal Cargo edge** to billing/payment/accounting (dev-deps only,
for the seam test). A composition layer implements them over the real services.

### Bounded context
The single business domain a module owns. One module = one bounded context. A module never edits
another's schema; it references other modules by logical FK.

### Composition root
`src/lib.rs` — the `PosModule` struct and `PosModuleBuilder`
(`builder().with_database(pool).build()?`). Wires the six generated CRUD services and composes the
routers. The one place allowed to depend on every layer. It lives in `src/lib.rs`; there is no separate
`module.rs`.

### CUSTOM marker
A `// <<< CUSTOM … // END CUSTOM` region inside a generated file. Content between the markers
survives regeneration. Spelling varies per file (`// <<< CUSTOM METHODS START >>>`, `// <<< CUSTOM
DTOs`, …) — match what is already there.

### DTO (Data Transfer Object)
A wire-shape struct in `src/application/dto/`. Per entity: `Create…Dto`, `Update…Dto`, `Patch…Dto`,
`…ResponseDto`, `…SummaryDto`, `…ListResponseDto`. Serialized `camelCase`. Generated, with
`From`/`Apply` conversions to and from the entity.

### Domain layer
The innermost layer (`src/domain/`): entities, value objects, enums, invariants, and repository
**traits** (ports). Depends on nothing.

### Drawer reconciliation
The cash-count check at close. Per payment method, `expected = opening_float + Σ recognised tenders`
(for cash, also `− Σ change_due`); `difference = counted − expected`; `difference_total = Σ`
over/short across methods. Persisted in the `PosClosingEntry.totals_by_method` JSON.

### Generation targets
The 31 kinds of artifact `metaphor schema schema generate` can emit (`rust`, `sql`, `dto`,
`handler`, `repository`, `service`, `proto`, `openapi`, …). `--target all` (default) emits the lot;
a comma-separated subset emits part.

### `GenericCrudRepository` / `GenericCrudService`
The `backbone-orm` / `backbone-core` generics that carry all standard CRUD. A module's repository is
a **newtype** over `GenericCrudRepository<Entity, SoftDelete>`; its service is a **type alias** over
`GenericCrudService<Entity, CreateDto, UpdateDto, Repository>`. Inherited, never re-implemented.

### Infrastructure layer
The adapter layer (`src/infrastructure/`): repository implementations, cache, messaging, jobs.
Depends on domain and application.

### Logical foreign key
A cross-module reference declared with `@foreign_key(module.Type.field)` (e.g.
`@foreign_key(sapiens.User.id)`). It documents the relationship and is *not* enforced by a database
constraint, so modules stay independently deployable.

### Logical FK targets
The specific external types POS references by logical FK (no DB constraint): `accounting.Account`
(the register's GL account refs), `billing.SalesInvoice` (`billing_invoice_id`),
`payment.PaymentEntry` (`payment_entry_id`), `party.Party` (customer), `catalog.Item` (line item),
`organization.Company`/`organization.Branch` (outlet), `sapiens.User` (audit `*_by`).

### `metaphor`
The workspace CLI (v0.2.0) that orchestrates the projects and dispatches to plugins
(`metaphor-schema`, `metaphor-codegen`, `metaphor-dev`). Prefer it over raw `cargo`/`sqlx`. Note:
there is **no** standalone `backbone-schema` binary (some older framework docs/skills still mention
one); use `metaphor schema schema …`.

### Module
A **library crate** owning one bounded context in 4-layer DDD, schema-driven. `[lib]` only — no
`main.rs`. Composed into a `backend-service`; never run alone. This repo is `backbone-pos`, one such module.

### Own schema (per module)
Each module gets its own Postgres schema (`schema: pos` in `index.model.yaml`). Migrations
`CREATE SCHEMA <module>` and qualify tables as `<module>.<table>`, so modules never collide on a
table name.

### Port / Adapter
The DDD names for the two halves of a repository: the **port** is the domain-layer `trait` (the
contract, e.g. the `PosInvoiceRepository` trait); the **adapter** is the infrastructure-layer
`struct` newtype over `GenericCrudRepository` (the Postgres implementation). See also
[BillingPort / PaymentPort](#billingport--paymentport) for the *outbound* orchestration ports.

### PosClosingEntry / Z-report
The cashier close (`pos_closing_entries`, table `pos.pos_closing_entries`). Carries
`totals_by_method` (per-method `{expected, counted, difference}`), `difference_total` (Σ over/short),
`grand_total`, `invoice_count`. Enum `PosClosingStatus`: `draft → submitted → reconciled`. Also called
the **Z-report**.

### PosEvent / PosEventSink
The domain-event union (`src/application/service/pos_events.rs`) and its sink — the module's **public
extension surface**. `PosEvent` variants: `PosSessionOpened`, `PosInvoicePaid` (carries the billing
invoice + payment), `PosSessionClosed` (carries `difference_total`), `PosInvoiceReturned`. Consumers
implement the `PosEventSink` trait (default `LoggingSink`); a loyalty/analytics consumer subscribes to
`PosInvoicePaid`.

### PosInvoice / Ticket
The counter sale (`pos_invoices`) — the **ticket**. Money fields: `net_total`, `tax_total` (PPN),
`grand_total = net + tax`, `rounded_total`, `rounding_adjustment`, `paid_total`, `change_due`.
`billing_invoice_id` links the real Sales Invoice billing raised; `is_return`/`return_against` mark a
return. Enum `PosInvoiceStatus`: `draft → paid` (recognised) / `void`; `consolidated` / `returned`.

### PosInvoiceItem / Sale line
One line on a ticket (`pos_invoice_items`) — a **sale line**. `quantity`, `unit_price`
(promo-resolved), `discount_amount`, `net_amount = money(qty·price) − discount`, optional
`revenue_account_id`.

### PosOpeningEntry / Session
The till-open, aka the **cashier session** (`pos_opening_entries`). `opening_balances` JSON = opening
float per method. Sales ring against an **open** session; enum `PosSessionStatus`: `open → closed`.

### PosPayment / Tender
One tender line against a ticket (`pos_payments`). `payment_method`, `amount`, `reference_no`,
`payment_entry_id` (the `payment.PaymentEntry` that settled it). Enum **`PosPaymentMethod`**: `cash`,
`card`, `qris`, `e_wallet`, `bank_transfer`, `virtual_account`.

### PosProfile / Register
The **register** configuration for an outlet (`pos_profiles`): name, currency (default IDR),
`allow_discount`, `is_active`, and the GL account refs the handoff needs — `income_account_id`,
`receivable_account_id`, `cash_account_id`, `write_off_account_id`, `default_customer_id` (walk-in).
Accounts are logical FKs to `accounting.Account`.

### `PosWriteService`
The hand-authored (`user_owned`) validated write path + retail orchestrator
(`src/application/service/pos_write_service.rs`). Owns `open_session`, `ring_sale`, `add_tender`,
`recognize_sale`, `return_sale`, `close_session` — all server-side money and every guard. Not
generated CRUD; survives regeneration.

### PPN
Indonesian VAT (Pajak Pertambahan Nilai). POS **carries** it (`PosInvoice.tax_total`) but does not
compute it — billing/tax does. Recognition **refuses** a ticket carrying PPN until the register's
profile has a tax account.

### Presentation layer
The transport layer (`src/presentation/`, `src/routes/`): HTTP handlers, route composition, and
optionally gRPC/GraphQL. Depends on the application layer.

### Recognition / recognize_sale / the retail seam
Turning a fully-tendered draft ticket into recognised revenue. `recognize_sale(id, &BillingPort,
&PaymentPort)` drives billing (`raise_and_post` → real Sales Invoice, `Dr A/R · Cr Revenue`),
persists `billing_invoice_id` while still draft (at-most-once — a decline-then-retry reuses it, no
double revenue), then payment (`settle` → `Dr Cash · Cr A/R`), flips `draft → paid`, and emits
`PosInvoicePaid` once. **POS posts no GL itself.** Idempotent (short-circuits on `paid`). The
cross-module handoff it performs is **the retail seam**.

### Regeneration (regen)
Re-running `metaphor schema schema generate … --force` to rebuild all downstream code from the
schema. Overwrites everything **outside** a protected region (CUSTOM markers, `*_custom.rs`,
`user_owned` globs).

### Return / return_sale
Reversing a recognised sale. `return_sale(id, &BillingPort, &PaymentPort)` reverses **both legs** —
`payment.refund` (`Dr A/R · Cr Cash`) + `billing.credit_note` (`Dr Revenue · Cr A/R`) — so revenue,
cash, and A/R net back to zero. Records an `is_return` ticket linked by `return_against`, flips the
original `paid → returned`, emits `PosInvoiceReturned`. Idempotent; full-ticket only.

### Ring (ring a sale)
`ring_sale(NewSale)` — create the **draft** ticket with all money computed **server-side**. Guards:
≥1 line, non-negative amounts, session open, unique receipt number.

### Schema (the SSoT)
`schema/models/*.model.yaml` — the single source of truth. Every entity struct, DTO, migration,
repository, service, handler, and route is generated from it. Not to be confused with the *Postgres
schema* (the per-module namespace).

### Server-side money / IDR receipt rounding
POS computes every amount itself; client-supplied totals are never trusted. Per line
`net = money(qty·price) − discount`; `grand = net + tax`; then IDR **receipt rounding**
`rounded = round_to(grand, step)` to the nearest step, **half-away-from-zero**, with
`rounding_adjustment = rounded − grand` (may be negative). Proven by `tests/pos_golden_cases.rs`
(the oracle).

### Soft delete
Marking a row deleted (`metadata.deleted_at` set) instead of removing it, enabled by
`config.soft_delete: true`. Backs the `soft_delete` / `restore` / `empty_trash` / `list_deleted`
endpoints.

### Tender / add_tender / change_due
A payment recorded against a draft ticket. `add_tender(id, method, amount, ref)` recomputes
`paid_total = Σ` and `change_due = max(0, paid − rounded)`; the ticket must be `draft` (else
`not_draft`); `fully_tendered = paid ≥ rounded`. **`change_due`** is cash overpayment handed back.

### Twelve endpoints
The standard CRUD surface every entity gets from `BackboneCrudHandler`: `list`, `create`, `get`,
`update`, `patch`, `soft_delete`, `restore`, `empty_trash`, `bulk_create`, `upsert`, `find_by_id`,
`list_deleted`.

### `user_owned`
The `metaphor.codegen.yaml` key listing glob paths the generator skips wholesale — never reads,
merges, or deletes. In POS it protects `docs/**` (this handbook), `tests/features/**`, and the
hand-authored heart: `PosWriteService`, the guarded routes, `pos_ports.rs`, and `pos_events.rs`.
