<!-- Reader: Evaluator · Mode: Explanation -->
# Background & prior art

Backbone modules did not appear from nowhere. Each design choice is a response to a real approach
that people use to build database-backed services — and to the specific way each one hurts at
scale. This page credits those approaches honestly and says what Backbone borrows and what it
rejects.

## The approaches that came before

### 1. Hand-rolled layers (the honest baseline)

Write the entity, DTOs, migration, repository, service, and handler by hand for every entity.

- **What's good:** total control, no magic, nothing to learn.
- **Where it breaks:** it does not scale past a handful of entities. Every entity re-litigates
  pagination, soft-delete, error shape, and bulk semantics, so they drift. The 5% of code that is
  real business logic is buried in 95% that is mechanical. Reviews are exhausting because every PR
  is mostly boilerplate.
- **Backbone keeps:** the explicit 4-layer structure — you can still read every generated file.
- **Backbone rejects:** *writing* the 95% by hand. It is generated from the schema instead.

### 2. Heavyweight ORMs / active-record frameworks

Rails, Django, Hibernate: a base class gives you CRUD, migrations, and query building.

- **What's good:** enormous leverage; CRUD is nearly free.
- **Where it breaks:** the magic is at *runtime*. The generated SQL is invisible until it misfires;
  the "fat model" quietly couples domain logic to persistence; type safety is weak or reflective.
- **Backbone keeps:** the leverage — generic CRUD you inherit rather than write.
- **Backbone rejects:** runtime magic and the domain/persistence coupling. Backbone generates
  *visible Rust source you can read and step through*, keeps the domain layer free of persistence
  concerns, and uses SQLx so queries are checked against the schema at **compile time**.

### 3. Schema-first codegen (OpenAPI, Prisma, protobuf)

Describe the data once; generate types/clients/servers.

- **What's good:** one source of truth, consistent artifacts, no drift *if* you never hand-edit.
- **Where it breaks:** the "never hand-edit" clause. The moment you need custom logic, most codegen
  tools force a choice: fork the output (and lose regeneration) or bolt logic on awkwardly outside.
- **Backbone keeps:** the single source of truth (the schema YAML) and full-artifact generation.
- **Backbone rejects:** the all-or-nothing edit boundary. The `// <<< CUSTOM` marker and
  `user_owned` files let generated and hand-written code *coexist in the same tree*, so you keep
  regenerating forever without losing your logic. ([ADR-0003](adr/adr-0003-custom-markers.md).)

### 4. Laravel-style scaffolders (`make:*`)

A generator writes starter files once; from then on they are yours to edit.

- **What's good:** fast start, familiar to many developers (Backbone even mirrors the ergonomics
  with `metaphor make entity`).
- **Where it breaks:** scaffolding is *one-shot*. After generation the files drift from any spec;
  there is no re-generation, so consistency erodes the moment two developers touch two entities.
- **Backbone keeps:** the ergonomic `make` entry point.
- **Backbone rejects:** the one-shot nature. Backbone's generation is *idempotent and repeatable* —
  the schema stays authoritative for the life of the module, not just at birth.

## What Backbone synthesizes

Backbone modules are the intersection of the four:

| From | Borrowed | Rejected |
|------|----------|----------|
| Hand-rolled layers | Explicit, readable 4-layer DDD structure | Writing the boilerplate by hand |
| Heavyweight ORMs | Inherited generic CRUD | Runtime magic; domain/DB coupling |
| Schema-first codegen | One source of truth; full-artifact generation | The all-or-nothing edit boundary |
| Laravel scaffolders | Ergonomic `make` entry point | One-shot, non-repeatable generation |

The result is a fifth thing: **repeatable, compile-time-checked, regen-safe scaffolding over a
strict DDD skeleton.** You get ORM-level leverage with hand-rolled-level transparency, and you can
regenerate for the life of the project without ever losing custom logic.

## Prior art for the POS itself

The four comparisons above are framework-level: they explain how a *module* is built. Retail POS
has its own lineage, and `backbone-pos` makes a specific bet against two common patterns.

### Monolithic POS that owns its own ledger

The traditional retail POS posts revenue, tax, and settlement *itself* — a general ledger that runs
parallel to the ERP's books. It rings, it recognises, it posts, all in one binary.

- **What's good:** the counter is self-contained; nothing downstream needs to be online to sell.
- **Where it breaks:** two ledgers is one ledger too many. The retail GL and the ERP's GL drift,
  and reconciliation between them becomes a permanent tax on the finance team. This is the classic
  source of "the POS numbers don't match the books."
- **backbone-pos rejects the parallel posting path.** It orchestrates the ERP's *existing* billing
  and payment emitters instead of re-implementing them, so there is exactly one GL contract — the
  same one web and B2B sales already post through. A cash sale nets A/R to zero at the counter, but
  the posting was done by billing and payment, not by POS.

### ERP POS modules coupled to one accounting engine

ERP-native counters (ERPNext POS and its kin) avoid the double-ledger problem honestly: the counter
calls straight into *the* accounting engine. One ledger, no drift.

- **What's good:** correct by construction — there is only one place that posts.
- **Where it breaks:** the counter is welded to one engine. The seam is an in-process call, so POS
  cannot be built, tested, or regenerated without the whole accounting stack present, and swapping
  or stubbing the emitter is awkward.
- **backbone-pos keeps the single-ledger correctness but makes the seam explicit.** Recognition
  goes through `BillingPort` / `PaymentPort` — an envelope + anti-corruption layer — so POS has
  **no Cargo edge to billing or payment** and each module regenerates independently. The §5
  round-trip test proves it: every seam port and ACL file is byte-identical after regeneration.

Credit where due: both patterns solve real problems, and the ERP-native approach in particular is
correct, not naive. backbone-pos simply wants single-ledger correctness *and* an independently
buildable counter, and the port seam is how it gets both.

### A region-neutral money stance

One more borrowed discipline worth naming: money is computed **server-side**, receipts are rounded
to the nearest IDR step (Indonesia-first, but the rounding is a profile setting, not a hard-coded
assumption), and tax is **carried, not computed** — POS moves the supplied PPN into the handoff and
lets billing/tax own the calculation. The counter stays a thin, honest orchestrator of money it did
not invent.

## Where it sits in the Metaphor workspace

A module is one project type among several the [Metaphor CLI](../schema/INTEGRATION.md) orchestrates:

- **`crate`** — a focused Rust library (one concern).
- **`module`** — *this* — a bounded domain library (4-layer DDD, schema-driven). **Consumed by
  services; never run alone.**
- **`backend-service`** — a runnable Axum/SQLx/Tonic server that *composes* modules.
- **`cli-tool`**, **`mobileapp`** — the edges of the system.

Backbone modules borrow their identity model from sibling modules (e.g. `sapiens` for `User`) by
logical reference, and are wired into services by a service's composition root. `backbone-pos` in
particular references several siblings purely by **logical foreign key** (no DB constraint): GL
accounts → `accounting`, the raised Sales Invoice → `billing`, the settling PaymentEntry →
`payment`, the walk-in customer → `party`, sold items → `catalog`, and company / branch →
`organization` — plus `sapiens` for identity. The
[Architecture](04-architecture.md) page shows exactly where the seams are.

---

Next: [Technology & the "why"](03-technology.md) — the stack, choice by choice.
