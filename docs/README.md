# backbone-pos — Handbook

The documentation set for **`backbone-pos`** — the **retail counter path of an Indonesia-first ERP's
Financials pillar**. POS owns the cashier session, the sale ticket, tender, and the drawer. It posts
**no general ledger itself**: on recognition it *orchestrates* `backbone-billing` (revenue) and
`backbone-payment` (settlement) through outbound ports, so retail reuses the same GL posts as web/B2B
sales. A cash sale nets A/R to zero at the counter.

> **Provenance.** This handbook descends from the [module skeleton][skel] and has been **adapted to the
> POS domain** — the pages below describe the real entities (PosProfile, PosOpeningEntry, PosInvoice
> (+items), PosPayment, PosClosingEntry), the retail seam, and the guarded HTTP surface, not the
> skeleton's placeholder `Example`. The framework-level chapters (philosophy, background, technology,
> maintainer workflow, contribution rules) still explain how *any* Backbone module works; the
> domain-level chapters (architecture, developer guide, glossary) are POS-specific.

[skel]: ../README.md

Every page below names **one reader** and **one mode** (Diátaxis) at its top. Find your reader, follow
the path.

## Find your path

| You are… | You want to… | Start here |
|----------|--------------|-----------|
| **Evaluator** | Decide whether to build on POS | [Philosophy](handbook/01-philosophy.md) → [Background](handbook/02-background.md) → [Technology](handbook/03-technology.md) |
| **App developer** | Ring a sale and integrate the module | [Developer Guide](handbook/06-developer-guide.md) |
| **Maintainer** | Understand the machine and extend it safely | [Architecture](handbook/04-architecture.md) → [Maintainer Guide](handbook/05-maintainer-guide.md) |
| **Contributor** | Open a correct PR | [Contributing](handbook/07-contributing.md) |
| **Anyone** | Agree on what a word means | [Glossary](handbook/08-glossary.md) |

## The handbook

1. [Philosophy & motivation](handbook/01-philosophy.md) — *Evaluator.* The problem POS solves, the "orchestrate, don't re-implement" north star, and the non-goals.
2. [Background & prior art](handbook/02-background.md) — *Evaluator.* Hand-rolled CRUD, ORMs, scaffolders — and how retail POS is usually built (owns-its-own-ledger, ERP-native) and what POS rejects.
3. [Technology & the "why"](handbook/03-technology.md) — *Evaluator + Maintainer.* The stack choice by choice, plus what matters for POS (`rust_decimal` money, the no-Cargo-edge ports).
4. [Architecture](handbook/04-architecture.md) — *Maintainer.* C4 view: context, containers, the DDD 4-layer shape, and the retail-sale seam traced end-to-end.
5. [Maintainer Guide](handbook/05-maintainer-guide.md) — *Maintainer.* Schema-YAML SSoT, regeneration, `// <<< CUSTOM` markers, the hand-authored write path + ports, release flow.
6. [Developer Guide](handbook/06-developer-guide.md) — *App developer.* Install → quickstart → walk a counter sale → recipes → configuration → troubleshooting.
7. [Contributing](handbook/07-contributing.md) — *Contributor.* Dev setup, commit/PR conventions, tests and lint, review checklist.
8. [Glossary](handbook/08-glossary.md) — *All.* One term, one meaning, used everywhere.
9. [Architecture Decision Records](handbook/adr/) — *Maintainer.* The framework decisions; plus the module's own [ADR-001 (POS boundary + retail seam)](adr/ADR-001-pos-boundary-and-retail-seam.md).

## Related, already-written docs

This handbook is the *narrative*. Reference sets live alongside it — link out, don't duplicate:

- **Product docs** — the [BRD](BRD.md) (business rules), [PRD](PRD.md) (problem + goals), and [FSD](FSD.md) (functional spec) for POS.
- **[End-to-end use cases & data transformation](USE_CASES.md)** — the cashier sale flow, custom/integration use cases, the ticket → invoice → settlement → GL field mappings, and how POS is consumed (with the retail-vs-ecommerce and why-no-catalog answers).
- **[Business flows](business-flows/README.md)** — the flows POS owns, each linked to its executable oracle; the [golden cases](business-flows/golden-cases.md) mirror `tests/pos_golden_cases.rs`, `tests/integrity_probes.rs`, and `tests/retail_sale_seam.rs` one-to-one.
- **[Schema DSL reference](schema/README.md)** — the exact YAML grammar: [types](schema/TYPES.md), [model rules](schema/RULE_FORMAT_MODELS.md), [generation targets](schema/GENERATION.md), [error codes](schema/ERROR_CODES.md), [examples](schema/EXAMPLES.md). The *Reference* corner of Diátaxis; the handbook explains the *why*.
- **[Module decision record](adr/ADR-001-pos-boundary-and-retail-seam.md)** — why POS owns the session + ticket, posts no GL, and orchestrates billing + payment (the retail seam).

## Conventions this handbook follows

- **Reader + mode named** at the top of every page.
- **Commands are real.** Every `metaphor …` command was run against `metaphor 0.2.0` while writing. Where a command in the top-level [README](../README.md) is stale (the standalone `backbone-schema` / `backbone` binaries), the handbook flags it and gives the working form.
- **Code wins over docs.** When a doc and the schema/code disagree, the schema YAML (the source of truth) wins — the doc is the bug.
