<!-- Reader: Maintainer · Mode: How-to -->
# Maintainer Guide

How to maintain this module and add features without breaking the regeneration machine. If you
only read one rule, read this one: **edit the schema YAML, then regenerate; put hand-written code
only where the generator promises not to touch it.**

All commands below were run against `metaphor 0.2.0`. Everything goes through `metaphor` — there is no
standalone `backbone-schema` / `backbone` binary (some older framework docs and skills still mention
one; ignore them).

## Before you touch anything

- Read this project's [`CLAUDE.md`](../../CLAUDE.md) and the workspace `metaphor.yaml`.
- Confirm the project type is **`module`** — that dictates every rule here. A module is a
  **library** (`[lib]` only). Never add a `main.rs` or a binary target.
- Internalize the source of truth: **`schema/models/<entity>.model.yaml`**. Code is downstream.
- Remember what POS *is*: the retail counter path. It owns the cashier session, the sale ticket,
  the tender, and the drawer. **It posts no general ledger itself** — on recognition it
  *orchestrates* `backbone-billing` (revenue) and `backbone-payment` (settlement) through outbound
  ports. "Orchestrate, don't re-implement" is the north star; keep it in mind before you add code.

## Where code goes (and what it may depend on)

| Layer | Directory | Put here | May depend on |
|-------|-----------|----------|---------------|
| Domain | `src/domain/` | Entities, value objects, invariants, repository **traits** | nothing |
| Application | `src/application/` | Services (type aliases), DTOs, use cases, the hand-authored write path | domain |
| Infrastructure | `src/infrastructure/` | Repository impls, cache, messaging, jobs | domain, application |
| Presentation | `src/presentation/`, `src/routes/` | HTTP handlers, route composition | application |

Dependency arrows point inward. If you find the domain layer importing `axum` or `sqlx`, something
is in the wrong layer.

## The plumbing vs. the heart

Almost all of `src/` is generated plumbing. From `schema/models/*.model.yaml` the pipeline emits,
for each of the six entities (`PosProfile`, `PosOpeningEntry`, `PosInvoice`, `PosInvoiceItem`,
`PosPayment`, `PosClosingEntry`), the entity struct, DTOs, migration, repository newtype, service
type alias, HTTP handler, and the twelve CRUD endpoints. That plumbing is real and you must keep it
compiling — but it is **not what you maintain**. Generic CRUD lets a well-formed request write a
ticket with inconsistent totals or reopen a closed session; it exists for trusted admin/seeding
only.

What maintainers actually maintain is the **hand-authored, user-owned heart** — the validated write
path and the retail seam. It already exists, and it is four files:

| File | What it is |
|------|-----------|
| [`src/application/service/pos_write_service.rs`](../../src/application/service/pos_write_service.rs) | `PosWriteService` — the validated write path + retail orchestrator. `open_session`, `ring_sale`, `add_tender`, `recognize_sale`, `return_sale`, `close_session`. Server-side money (`net`, `grand`, IDR `rounded_total`, `rounding_adjustment`, `paid_total`, `change_due`), the validation gates, and the billing/payment handoff live here. |
| [`src/application/service/pos_ports.rs`](../../src/application/service/pos_ports.rs) | The `BillingPort` / `PaymentPort` seam — outbound orchestration traits + their serde request/ack structs. This is how POS drives two downstream emitters **without a Cargo dependency on either**. |
| [`src/application/service/pos_events.rs`](../../src/application/service/pos_events.rs) | The `PosEvent` union (`PosSessionOpened`, `PosInvoicePaid`, `PosSessionClosed`, `PosInvoiceReturned`), the `PosEventSink` trait, and the default `LoggingSink`. The public extension surface for loyalty/analytics consumers. |
| [`src/presentation/http/guarded_routes.rs`](../../src/presentation/http/guarded_routes.rs) | `create_guarded_pos_routes` — the recommended mount. Read documents + validated writes (`POST /pos-sessions`, `/pos-sales`, `/pos-tenders`, `/pos-sessions/close`) wrapping `PosWriteService`. Generic CRUD mutation is deliberately **not** mounted. |

These four files carry the module's business logic. They are protected wholesale via `user_owned`
in [`metaphor.codegen.yaml`](../../metaphor.codegen.yaml) — the generator never reads, merges, or
deletes them. See [Regen-safety](#regen-safety--the-rules-that-keep-your-logic-alive) below for the
exact globs.

## Adding a new entity (the golden path)

Say you want a `PosDiscountRule` — a per-profile promotion rule you can attach to a register. It is
plumbing: an ordinary CRUD entity in the `pos` schema. Schema first, always.

```bash
# 1. Describe it. Copy an existing model as a starting point…
cp schema/models/pos_profile.model.yaml schema/models/pos_discount_rule.model.yaml
#    …edit the entity (name PosDiscountRule, table pos_discount_rules, fields), then add it to the
#    index so the generator picks it up:
#      imports:
#        - pos_discount_rule.model.yaml   ← add under `imports:` in schema/models/index.model.yaml

# 2. Validate the schema before generating.
metaphor schema schema validate pos

# 3. Generate all artifacts (entity, DTOs, repo, service, handler, routes).
metaphor schema schema generate pos --target all --force

# 4. Generate the migration for the new entity.
metaphor migration generate PosDiscountRule pos

# 5. Apply migrations.
metaphor migration run

# 6. Register the service in the composition root — src/lib.rs (see below), then:
metaphor dev test
```

> `pos` is this module's name (auto-detected from the current directory when omitted).
> `--target` accepts a comma-separated subset if you want to regenerate just part of the cake
> (e.g. `--target dto,handler`). Run `metaphor schema schema generate --help` for the full target
> list. Use `--dry-run` first if you want to see what would change without writing.
> `graphql`, `grpc`, and `proto` generators are disabled in `index.model.yaml` for this module —
> `--target all` will not emit them.

### Step 6 in detail — wire the service into `PosModule` (in `src/lib.rs`)

Generation does **not** edit the composition root for you. The composition root for this module is
**`PosModule` + `PosModuleBuilder` in [`src/lib.rs`](../../src/lib.rs)** — there is no separate
`module.rs`.

> **Note.** Older framework docs (and the skeleton version of this page) told you to wire services into
> a `Module` struct in `src/module.rs` and mount `Module::http_routes()`. That file does not exist in
> this module — `PosModule` in `src/lib.rs` is the one and only composition root.

Follow the existing `pos_*` service pattern exactly. Every generated service is stored as an
`Arc<XService>` built via `XService::with_repository(XRepository::new(db_pool.clone()))`, and the
new field/build/return lines all sit **inside `// <<< CUSTOM` markers** so regeneration preserves
them:

```rust
// in PosModule (the struct):
pub struct PosModule {
    pub pos_closing_entry_service: Arc<PosClosingEntryService>,
    pub pos_invoice_service: Arc<PosInvoiceService>,
    // … the other four …
    // <<< CUSTOM
    pub pos_discount_rule_service: Arc<PosDiscountRuleService>,   // ← add the field
    // END CUSTOM
}

// in PosModuleBuilder::build(), inside the `// <<< CUSTOM` block that follows the generated services:
    // <<< CUSTOM
    let pos_discount_rule_repository = Arc::new(PosDiscountRuleRepository::new(db_pool.clone()));
    let pos_discount_rule_service    = Arc::new(PosDiscountRuleService::with_repository(pos_discount_rule_repository.clone()));
    // END CUSTOM

// in the returned PosModule { … }, inside its trailing `// <<< CUSTOM` block:
        // <<< CUSTOM
        pos_discount_rule_service,   // ← return it
        // END CUSTOM
```

Then re-export the service from [`src/lib.rs`](../../src/lib.rs) alongside the existing
`pub use application::service::PosProfileService;` lines, and, if you want the generic CRUD mounted,
add `create_pos_discount_rule_routes(...)` to `PosModule::all_crud_routes()`. For a **guarded**
surface, mount it in `guarded_routes.rs` instead (see below) — generic CRUD is intentionally left
off the recommended mount.

## Changing an existing entity

1. Edit the field in `schema/models/<entity>.model.yaml` (the SSoT — never the generated struct).
2. `metaphor schema schema validate pos`.
3. Generate a migration for the change:
   `metaphor migration generate <Entity> pos`.
4. Regenerate code: `metaphor schema schema generate pos --target all --force`.
5. `metaphor migration run && metaphor dev test`.

If the change touches money fields, the ticket lifecycle, or a GL account ref on `PosProfile`, the
hand-authored `PosWriteService` almost certainly needs a matching edit — the generator will not
touch it, and it holds the arithmetic and the validation gates. Re-run the golden cases
(`tests/pos_golden_cases.rs`) and the seam tests (`tests/retail_sale_seam.rs`) after any such
change.

## Regen-safety — the rules that keep your logic alive

Regeneration **overwrites everything outside a protected region.** There are three protected
mechanisms; know which one you are using.

### 1. `// <<< CUSTOM … // END CUSTOM` markers (inside generated files)

The generator preserves whatever sits between the markers. `src/lib.rs` ships several ready to fill
— the builder-method slot, the `build()` body, and the struct-return slot are all the way you wired
`PosDiscountRuleService` above:

```rust
// in PosModuleBuilder::build()
// <<< CUSTOM
// END CUSTOM
```

Marker spellings vary slightly by file — the entity uses `// <<< CUSTOM METHODS START >>>` /
`// <<< CUSTOM METHODS END >>>`, the DTO file uses `// <<< CUSTOM DTOs` / `// >>> END CUSTOM DTOs`.
**Match the spelling already in the file**; add your code between the existing pair, do not invent
new marker text.

Use markers for small additions: a helper method on the entity, an extra DTO, a service
registration, a re-export.

### 2. `*_custom.rs` sibling files (never generated, never overwritten)

For anything substantial that still belongs *next to* a generated service, write a whole file the
generator never emits and so never touches:

```rust
// application/service/pos_invoice_service_custom.rs   ← the generator will never write this name
use std::sync::Arc;
use crate::application::service::PosInvoiceService;

pub struct PosInvoiceServiceCustom {
    inner: Arc<PosInvoiceService>,
    // domain-specific deps
}
// … your business rules …
```

Register it from the surrounding `mod.rs` **inside a `// <<< CUSTOM` marker** so the `mod`
declaration survives regeneration too.

### 3. `user_owned` globs in `metaphor.codegen.yaml`

[`metaphor.codegen.yaml`](../../metaphor.codegen.yaml) lists paths the generator skips **wholesale**
— never reads, merges, or deletes. This is how the module's heart survives. The actual list today:

```yaml
user_owned:
  - "src/application/service/pos_events.rs"
  - "src/application/service/pos_ports.rs"
  - "src/application/service/pos_write_service.rs"
  - "src/presentation/http/guarded_routes.rs"
  - "tests/pos_golden_cases.rs"
  - "tests/integrity_probes.rs"
  - "tests/retail_sale_seam.rs"
  - "scripts/**"
  - "tests/features/**"
  - "docs/**"
```

When you add a new hand-authored file — a new custom service, a report handler, a new seam port
module — add its path here in the same shape, so a whole path becomes immune to generation.

**Which to reach for:** a few lines (a service registration, an extra method) → a CUSTOM marker; a
cohesive unit of logic that sits beside generated code → a `*_custom.rs` file; an entire
hand-owned file or subtree (the write path, the ports, the guarded routes) → a `user_owned` glob.

## Adding a non-CRUD endpoint

The twelve CRUD endpoints come from `BackboneCrudHandler`. Everything the counter actually does — a
validated write, an action, a report — lives **outside** the generated handler, in the guarded route
layer this module already uses. Follow the shape in
[`guarded_routes.rs`](../../src/presentation/http/guarded_routes.rs):

1. Add the handler fn there (or in a sibling `*_custom.rs` under `presentation/http/`). Deserialize
   a camelCase body, call a method on `PosWriteService` (or a read service), and map `PosError` to
   an HTTP status via its `http_status()` / `code()`.
2. Add the route to `write_routes(...)` and let `create_guarded_pos_routes(&PosModule, pool)` compose
   it — *alongside* the read routes, never inside a `BackboneCrudHandler` merge.
3. The file is already in `user_owned`; a new file needs its own glob (see above).

For example, a Z-report-summary endpoint would add `GET /pos-sessions/{id}/z-report` here, backed by
a read query, and stay entirely out of generation. Never hand-roll a route that duplicates a CRUD
endpoint — extend the guarded layer, don't replace the generated one.

> Two write operations are deliberately **not** HTTP routes: `recognize_sale` and `return_sale`.
> Both need the `BillingPort` / `PaymentPort`, which a composition layer supplies, so they are
> service/job-driven. Keep them that way — don't expose a raw route that fabricates the ports.

## Adding a cross-module orchestration (the ports pattern)

This is the module's signature extension. When POS must **drive another module** — the way
`recognize_sale` drives billing (raise + post the Sales Invoice) and payment (settle the tender) —
you do **not** add a Cargo dependency on that module. You add a **port**.

The pattern, in [`pos_ports.rs`](../../src/application/service/pos_ports.rs):

1. Define a `#[async_trait]` trait (`BillingPort`, `PaymentPort`) with the operations POS needs.
2. Define plain `serde` request/ack structs for each operation — the wire contract
   (`SaleInvoiceRequest → InvoiceAck`, `SettlementRequest → SettlementAck`, `CreditNoteRequest` /
   `RefundRequest → ReversalAck`, and `PosRejected { code, message }` for the failure path).
3. `PosWriteService` takes the port as a `&dyn` argument and calls it; it never names the other
   module.
4. A **composition layer** (in the composing `backend-service`, not here) implements the trait over
   the real `backbone-billing` / `backbone-payment` services.

The shipped POS library therefore has **zero normal Cargo edge** to billing, payment, or accounting
(a dev-dependency exists only for the seam round-trip test). This is the envelope + anti-corruption
seam discipline — apply the same shape for any new downstream emitter POS needs to drive. The seam
is proven by `tests/retail_sale_seam.rs` and `scripts/retail_sale_seam_roundtrip.sh`; run them after
any port change, and confirm §5 of the seam test (all port/ACL files byte-identical after a regen
round-trip) still passes.

## Build, test, lint

```bash
metaphor dev test          # unit + integration + E2E for this module
metaphor lint check        # clippy + fmt policy
```

Never run bare `cargo build`/`cargo test` from the workspace root — each project has its own
`Cargo.toml`; use the `metaphor` wrappers so workspace policy applies. Inside *this* module
directory, `cargo test` works but `metaphor dev test` is preferred. The tests that matter most when
you change behavior:

- `tests/pos_golden_cases.rs` — ring totals, IDR rounding, multi-tender + change, close
  reconciliation, validation gates.
- `tests/integrity_probes.rs` — the at-most-once billing invariant (a decline-then-retry raises
  revenue exactly once), `not_fully_tendered`, `missing_account`, recognition idempotency.
- `tests/retail_sale_seam.rs` — the full POS↔billing↔payment↔accounting round-trip: A/R nets to
  zero on a cash sale, a return reverses both legs, and the seam files survive regeneration.

## Versioning & release

- This crate is versioned in [`Cargo.toml`](../../Cargo.toml). Bump per conventional-commits:
  `fix:` → patch, `feat:` → minor, `feat!:`/`BREAKING CHANGE` → major.
- Before releasing: `metaphor dev test` and `metaphor lint check` clean.
- Pin the `backbone-*` git deps to a tag/rev for any release build (see [Technology](03-technology.md)).
- Commits use conventional commits and carry **no Claude / co-author signature** — see
  [Contributing](07-contributing.md).

## What will break things

- **Editing generated code outside a CUSTOM marker** — silently overwritten on the next
  `generate --force`. This is the number-one regression.
- **Looking for `src/module.rs`** — it does not exist here; `PosModule` in `src/lib.rs` is the
  composition root. Wire services there.
- **Adding `main.rs` / a binary target** — wrong project type; a module is a library.
- **Hand-rolled Axum CRUD** — always use `BackboneCrudHandler`, and guard writes through
  `PosWriteService`, not raw routes.
- **Adding a Cargo dependency on billing / payment / accounting** — POS orchestrates through ports;
  a direct edge breaks the seam discipline (and the regen round-trip test).
- **Skipping the schema** and writing entity + migration + handler by hand — breaks regeneration
  forever after.
- **Touching a sibling module's schema** — one module owns one bounded context; reference other
  modules by logical FK, never edit theirs.

---

Next: [Developer Guide](06-developer-guide.md) if you are integrating a module rather than maintaining one.
