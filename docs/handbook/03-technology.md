<!-- Reader: Evaluator + Maintainer · Mode: Explanation -->
# Technology & the "why"

Every dependency in [`Cargo.toml`](../../Cargo.toml) earns its place. This page gives each
significant choice a one-line rationale and names the alternative that was rejected, so an
evaluator can judge the stack and a maintainer knows *why* not to swap a piece out casually.

The versions below are what `backbone-pos` **v0.1.3** pins in its own [`Cargo.toml`](../../Cargo.toml);
where behavior is version-specific, the version is called out.

## The choices

| Layer | Choice | Why | Rejected alternative |
|-------|--------|-----|----------------------|
| Language | **Rust 2021**, `[lib]` only | Memory safety + a type system strong enough to make generated code *provably* consistent; no GC pauses in a service hot path | Go (weaker types for the generated-DTO story), Kotlin (already used for the mobile edge, not the domain core) |
| Async runtime | **Tokio 1.x** (`full`) | The de-facto async runtime; Axum and SQLx are both built on it, so there is one reactor | `async-std` (smaller ecosystem, no Axum/SQLx alignment) |
| HTTP | **Axum 0.7** (+ `tower`, `tower-http`) | Tower middleware ecosystem, first-class extractors, and it composes as a plain `Router` — exactly what `BackboneCrudHandler` returns and the module merges | `actix-web` (its own actor model fights the compose-a-Router design) |
| Database | **PostgreSQL** via **SQLx 0.8** (`postgres`, `uuid`, `chrono`, `json`, `migrate`, `rust_decimal`) | Queries are **checked at compile time** against the schema — the codegen's consistency guarantee extends all the way to SQL; native enum, `uuid`, `jsonb`, and `NUMERIC`↔`Decimal` support | Diesel (heavier macro layer, less async-native), an ORM with runtime-only query building |
| Domain errors | **`thiserror` 1.0** | Ergonomic, zero-cost typed errors for the domain/service layers; the generated handler maps them to HTTP status + a stable error code | `anyhow` for domain errors (loses the typed variants the handler matches on) |
| Boundary errors | **`anyhow` 1.0** | Right tool at the *composition* boundary (`ModuleBuilder::build` returns `anyhow::Result`) where a typed enum adds no value | `thiserror` everywhere (ceremony with no payoff at the boundary) |
| Serialization | **`serde` / `serde_json`** | Universal; DTOs derive `Serialize`/`Deserialize` and `#[serde(rename_all = "camelCase")]` gives a stable JSON wire shape | manual (de)serialization (error-prone, defeats codegen) |
| IDs / time / money | **`uuid` v4**, **`chrono`**, **`rust_decimal`** | UUID primary keys avoid enumeration and merge cleanly across modules; `chrono` for audit timestamps; `rust_decimal` shipped by default because any `decimal` schema field generates code that imports it | integer PKs (leak ordinality, collide across modules), `f64` money (rounding bugs) |
| Config | **`config` 0.14** + **`serde_yaml`** | Layered YAML (`application.yml` + env overrides) matches the `config/` convention; `DATABASE_URL` overrides at runtime | hardcoded config, bespoke env parsing |
| Validation | **`validator` 0.16** (feature-gated) | DTO field rules (`@length(max=200)` → `#[validate(length(max = 200))]`) are declared in the schema and enforced at the edge | hand-written guard clauses scattered across handlers |
| gRPC / proto | **`tonic` 0.12** + `prost` 0.13 (`tonic-build` 0.12 build-dep) + `buf.yaml` | Skeleton's optional second transport. **Not enabled for backbone-pos** — the `grpc`, `proto`, and `graphql` generators are `disabled` in [`index.model.yaml`](../../schema/models/index.model.yaml); the crates are present but the POS module ships REST-only | — (POS deliberately runs one transport; see below) |
| Logging | **`tracing`** (+ `tracing-subscriber`) | Structured, async-aware spans; the service host installs the subscriber | `log` (no span/async context) |

## The framework crates

Four crates carry the leverage. In this skeleton they are **git dependencies** on the public
framework repo, pinned to `branch = "main"`:

```toml
backbone-core      = { git = "https://github.com/faridlab/backbone-framework", branch = "main", features = ["postgres"] }
backbone-orm       = { git = "https://github.com/faridlab/backbone-framework", branch = "main" }
backbone-auth      = { git = "https://github.com/faridlab/backbone-framework", branch = "main" }
backbone-messaging = { git = "https://github.com/faridlab/backbone-framework", branch = "main" }
```

| Crate | Gives the module | Seen in the skeleton as |
|-------|------------------|-------------------------|
| **`backbone-core`** | `GenericCrudService`, `BackboneCrudHandler`, `PersistentEntity`, `FromCreateDto` / `ApplyUpdateDto`, `ServiceError` / `ServiceResult` | the service type alias, the handler, DTO conversions, `service/error.rs` |
| **`backbone-orm`** | `GenericCrudRepository`, `SoftDelete`, `EntityRepoMeta`, pagination types | the repository newtype, the entity's `EntityRepoMeta` impl |
| **`backbone-auth`** | identity / permission primitives | reserved for the `permission/` and `auth/` layers |
| **`backbone-messaging`** | message-bus adapters | reserved for the `messaging/` layer |

> **Reproducibility note.** `branch = "main"` is convenient but *not reproducible* — a fresh
> `cargo build` can pull a newer commit. For anything you ship, pin to a tag or commit:
> `tag = "vX.Y.Z"` or `rev = "<sha>"`. `Cargo.lock` is committed, which pins transitively, but the
> git ref is what a `cargo update` will move.

> **Dependency form.** The `backbone-*` crates are **git dependencies** pinned to `branch = "main"`
> (the `Cargo.toml` comment notes this lets the crate "work anywhere on disk without path fix-up"),
> *not* path deps. For a release, pin to a tag or commit as above. The top-level
> [README](../../README.md) states this correctly.

## Choices that matter for POS specifically

The rows above are framework-level. Three choices are *load-bearing* for a retail counter that
computes money and hands recognition to two downstream emitters — they are not incidental.

### `rust_decimal` — money is exact, server-side, never `f64`

POS totals are computed on the server in `PosWriteService`, never trusted from the client and never
in floating point. Every money field is `@precision(18,2)` → `NUMERIC(18,2)` in Postgres, carried in
Rust as `rust_decimal::Decimal` (the `rust_decimal` sqlx feature is what makes the `NUMERIC`↔`Decimal`
round-trip compile-time-checked). Two rounding rules ride on top:

- `money(v)` rounds to **2 decimal places, half-away-from-zero** — applied per line
  (`net_amount = money(qty·unit_price) − discount`) and to the ticket `grand_total = net + tax`.
- IDR **receipt rounding** rounds `grand_total` to the nearest step via `round_to(grand, step)`
  (again half-away-from-zero), producing `rounded_total` and
  `rounding_adjustment = rounded_total − grand_total` (which may be negative). Golden case PGC-2
  pins both directions: `95,040 → 95,000` (adj `−40`) and `95,060 → 95,100` (adj `+40`).

This is why the precision and the rounding strategy are part of the contract, not implementation
detail: `f64` money would drift, and the over/short reconciliation at close (`counted − expected`)
would never land on zero. **Rejected:** `f64`/native float money (rounding bugs, non-reproducible
totals).

### The ports pattern — no Cargo edge to billing/payment/accounting

POS posts **no GL itself**. On recognition it *orchestrates* two downstream emitters through
hand-authored outbound traits in [`pos_ports.rs`](../../src/application/service/pos_ports.rs):

- `BillingPort { raise_and_post, credit_note }` — drives `backbone-billing` (real Sales Invoice,
  `Dr A/R · Cr Revenue`).
- `PaymentPort { settle, refund }` — drives `backbone-payment` (`Dr Cash · Cr A/R`).

The critical fact: **the shipped POS library has ZERO normal Cargo edge to billing/payment/
accounting.** Those crates appear in `Cargo.toml` only as `[dev-dependencies]` (`path = "../backbone-*"`),
used solely by the retail-sale seam test to drive the *real* services through in-test ACL adapters.
You can verify — `cargo tree -e normal -i backbone-billing` (and `-payment`, `-accounting`) is empty.

Why hand-authored traits instead of a direct dependency? Because a Cargo edge would couple the four
crates: billing or payment could no longer regenerate independently, and the general-ledger contract
would fork (retail posting its own GL beside web/B2B). Trait-plus-serde structs keep the seam an
*envelope*, so a composition layer implements the ports over whichever concrete services are wired,
and all four modules regenerate on their own clock. The §5 seam round-trip test asserts every
port/ACL file stays byte-identical across regeneration. **Rejected:** a direct Cargo dependency on
`backbone-billing`/`backbone-payment` (couples the crates, breaks independent regen, duplicates the
GL contract).

### `serde` on the ports and events — the wire contract

The port request/ack structs (`SaleInvoiceRequest → InvoiceAck`, `SettlementRequest → SettlementAck`,
`CreditNoteRequest`/`RefundRequest → ReversalAck`, `PosRejected { code, message }`) and the
[`pos_events.rs`](../../src/application/service/pos_events.rs) `PosEvent` union (`PosSessionOpened`,
`PosInvoicePaid`, `PosSessionClosed`, `PosInvoiceReturned`) all derive `Serialize`/`Deserialize`.
That is not decoration: these are the **wire contract** a composition layer (or a message bus behind
`PosEventSink`) carries between modules. `serde` earns its place here for the same reason it does for
DTOs — it is the stable, versionable envelope shape crossing a boundary.

## The CLI: `metaphor`, not `backbone-schema`

Generation, migration, and testing go through the **`metaphor`** binary (v0.2.0 at time of
writing), which dispatches to plugins (`metaphor-schema`, `metaphor-codegen`, `metaphor-dev`).

> **Note.** There is **no** standalone `backbone-schema` / `backbone` binary on `PATH` — some older
> framework docs and skills still invoke one. Everything goes through `metaphor` (e.g.
> `metaphor schema schema generate …`, `metaphor migration run`). The
> [Developer Guide](06-developer-guide.md) and [Maintainer Guide](05-maintainer-guide.md) use the
> verified commands throughout.

Why a workspace CLI instead of raw `cargo`/`sqlx`? Because a module never lives alone — it is one
project in a multi-project workspace, and `metaphor` applies workspace-wide policy (affected-only
builds, cross-project codegen, plugin discovery). See [ADR-0002](adr/adr-0002-generic-crud.md) for
the generic-CRUD decision and the schema docs' [INTEGRATION](../schema/INTEGRATION.md) for how the
pieces compose.

---

Next: [Architecture](04-architecture.md) — the C4 view and a request traced end-to-end.
