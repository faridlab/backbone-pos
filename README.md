# backbone-pos

The **retail counter path** of an Indonesia-first ERP's Financials pillar. A Backbone **domain module**
(a `[lib]`-only crate, 4-layer DDD, schema-YAML as the source of truth) that owns the cashier session,
the sale ticket, tender, and the drawer.

**It posts no general ledger itself.** On recognition it *orchestrates* two downstream emitters through
outbound ports — `backbone-billing` (raise + post the real Sales Invoice → revenue) and
`backbone-payment` (settle the tender) — so retail reuses the same GL posts as web/B2B sales. A cash
sale books `Dr A/R · Cr Revenue` (billing) then `Dr Cash · Cr A/R` (payment); **A/R nets to zero at the
counter.**

## Entities

Defined in [`schema/models/`](schema/models/) — the single source of truth, in the `pos` Postgres schema:

| Entity | Table | Role |
|--------|-------|------|
| **PosProfile** | `pos_profiles` | Register configuration: outlet, currency (IDR), and the GL account refs the handoff needs. |
| **PosOpeningEntry** | `pos_opening_entries` | A cashier session (till open) with opening float per method. Sales ring against an *open* session. |
| **PosInvoice** (+ **PosInvoiceItem**) | `pos_invoices` / `pos_invoice_items` | The ticket: server-side money, IDR receipt rounding, `billing_invoice_id`, returns. |
| **PosPayment** | `pos_payments` | A tender line (cash / card / QRIS / e-wallet / bank transfer / virtual account). |
| **PosClosingEntry** | `pos_closing_entries` | The Z-report: per-method expected-vs-counted drawer reconciliation. |

Cross-module ids (accounting, billing, payment, party, catalog, organization, sapiens) are **logical
foreign keys** — no DB constraint — so the modules stay independently deployable.

## Quickstart

Requires **Rust 2021**, the **`metaphor`** CLI (`0.2.0`+) on `PATH`, and a reachable **PostgreSQL**.

```bash
export DATABASE_URL="postgresql://root:password@localhost:5432/posdb"

metaphor schema schema validate      # check the schema YAML
metaphor migration run               # create the `pos` schema + tables
metaphor dev test                    # golden cases + integrity probes + retail-sale seam
metaphor lint check                  # clippy + fmt
```

> The `backbone-*` framework crates are **git dependencies** pinned to `branch = "main"` (see
> [`Cargo.toml`](Cargo.toml)), so the crate builds anywhere on disk with no path fix-up. For a release,
> pin them to a tag or commit (`tag = "vX.Y.Z"` / `rev = "<sha>"`) for reproducibility.

## Mounting the module

`backbone-pos` is a library — a `backend-service` composes it and mounts its router. Prefer the
**guarded** surface (read documents + validated writes; generic CRUD mutation is not mounted):

```rust
let pos = PosModule::builder().with_database(pool.clone()).build()?;
let router = backbone_pos::presentation::http::create_guarded_pos_routes(&pos, pool.clone());
```

Validated write routes: `POST /pos-sessions` (open), `/pos-sales` (ring), `/pos-tenders` (add tender),
`/pos-sessions/close`. Sale **recognition** and **returns** drive billing + payment through the
`BillingPort`/`PaymentPort`, so they are service/job-driven, not HTTP routes — see
[`tests/retail_sale_seam.rs`](tests/retail_sale_seam.rs) for a working composition.
`PosModule::all_crud_routes()` exposes the full **unguarded** CRUD surface for trusted/admin/seeding only.

## Documentation

Start with the **[handbook](docs/README.md)** — philosophy, background, technology, architecture,
maintainer guide, developer guide, contribution guide, glossary, and ADRs, each written for a named
reader.

- **[Developer Guide](docs/handbook/06-developer-guide.md)** — install → quickstart → walk a counter sale.
- **[Architecture](docs/handbook/04-architecture.md)** — the C4 view and the retail-sale seam traced end-to-end.
- **[Maintainer Guide](docs/handbook/05-maintainer-guide.md)** — schema-YAML SSoT, regeneration, the hand-authored write path + ports.
- **[Product docs](docs/PRD.md)** · **[Business flows](docs/business-flows/README.md)** · **[ADR-001 (POS boundary + retail seam)](docs/adr/ADR-001-pos-boundary-and-retail-seam.md)**.

## The single source of truth

`schema/models/*.model.yaml` defines every entity. The codegen pipeline produces the entity struct,
DTOs, migration, repository, service, generated CRUD handler, and routes. **Regeneration preserves only
code inside `// <<< CUSTOM … // END CUSTOM` markers, `*_custom.rs` files, and `user_owned` paths** (in
[`metaphor.codegen.yaml`](metaphor.codegen.yaml)) — which protect the hand-authored heart:
`pos_write_service.rs` (the validated write path + retail orchestrator), `pos_ports.rs`
(`BillingPort`/`PaymentPort`), `pos_events.rs` (the event union + sink), and `guarded_routes.rs`.

Edit the schema first; never hand-edit generated code outside a protected region. See the
[Maintainer Guide](docs/handbook/05-maintainer-guide.md) for the full workflow.
