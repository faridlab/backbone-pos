<!-- Reader: Contributor · Mode: How-to -->
# Contributing

How to land a change in a Backbone module — dev setup, conventions, and the checklist a reviewer
will hold you to. The single hardest rule to remember: **commit messages carry no Claude or
co-author signature.** Everything else is standard.

## Dev setup

```bash
# 1. Toolchain
rustup show                 # Rust 2021 edition toolchain
metaphor --version          # metaphor 0.2.0+ on PATH

# 2. A database for tests
export DATABASE_URL="postgresql://root:password@localhost:5432/skeletondb"
metaphor migration run

# 3. Prove a clean baseline before you change anything
metaphor dev test
metaphor lint check
```

If `metaphor` is not installed, see the workspace root `metaphor.yaml` / plugin discovery order
(`$PATH` → `$METAPHOR_PLUGIN_BIN_DIR` → `~/.metaphor/bin/`).

## The golden rule of module changes

You are almost never editing generated Rust directly. Before writing code, ask: *does this belong
in the schema?* If it changes an entity's shape, the answer is yes — edit
`schema/models/*.model.yaml`, regenerate, and commit the regenerated output together with the
schema change. A PR that hand-edits a generated struct will be sent back. See the
[Maintainer Guide](05-maintainer-guide.md) for the generate/regen workflow.

## Branch & commit conventions

- **Branch** off `main`. Never commit directly to `main`.
- **Conventional commits.** `type(scope): summary` — e.g.
  `feat(pos-invoice): reject tender on a non-draft ticket`,
  `fix(pos-write): correct IDR rounding half-away-from-zero`,
  `docs(handbook): adapt architecture to the retail seam`. Types drive
  versioning: `fix:` → patch, `feat:` → minor, `feat!:` / `BREAKING CHANGE:` → major.
- **One concern per commit.** Group by functionality; keep large generated files in their own
  commit rather than mixed with hand-written logic.
- **Message says *why*, not "update".** No filler (`wip`, `fix stuff`, `changes`).
- **NO signatures.** Never append `Co-Authored-By`, `Generated with…`, or any trailer. This is a
  hard workspace rule (root `CLAUDE.md`).

```
feat(pos-invoice): reject tender on a non-draft ticket

Business rule from business-flows/golden-cases.md (PGC-5): add_tender
only applies to a draft ticket, else `not_draft`.
Enforced in PosWriteService (user_owned) so it survives regeneration.
```

## Before you open a PR — the checklist

- [ ] Change started in the **schema YAML** if it touches an entity's shape.
- [ ] `metaphor schema schema validate` passes.
- [ ] Regenerated code committed alongside the schema change (no hand-edits outside CUSTOM regions).
- [ ] Custom logic lives in a `// <<< CUSTOM` marker, a `*_custom.rs` file, or a `user_owned` path.
- [ ] No `main.rs` / binary target added (this is a **library**).
- [ ] No hand-rolled Axum CRUD — `BackboneCrudHandler` used for standard endpoints.
- [ ] No sibling module's schema touched; cross-module references are logical FKs.
- [ ] Cross-module orchestration goes through a port in `pos_ports.rs` (serde request/ack, no Cargo
      edge), never a direct dependency on billing/payment/accounting.
- [ ] `metaphor dev test` green.
- [ ] `metaphor lint check` clean.
- [ ] New/changed behavior has a test; if it is a bug fix, a test that fails without the fix.
- [ ] Migrations have both `*.up.sql` and `*.down.sql`.
- [ ] Docs updated if behavior changed (this handbook, or the schema reference under `docs/schema/`).
- [ ] Conventional-commit messages, **no signatures**.

## Tests

- Unit + integration + E2E run through `metaphor dev test`.
- The real suites, and what each guards:
  - `tests/pos_golden_cases.rs` — the **numeric oracle**: ring totals, IDR rounding up/down,
    multi-tender + change, close reconciliation, validation gates (PGC-1…5).
  - `tests/integrity_probes.rs` — the seam invariants: `not_fully_tendered`, `missing_account`,
    recognition idempotency, decline-then-retry raising billing exactly once (IP-1…4).
  - `tests/retail_sale_seam.rs` + `scripts/retail_sale_seam_roundtrip.sh` — the **four-module seam**
    (POS↔billing↔payment↔accounting: A/R nets to zero, return reverses both legs) plus the §5 regen
    round-trip that asserts the seam port/ACL files stay byte-identical after `generate --force`.
- Business flows live in [`docs/business-flows/`](../business-flows/README.md) — `golden-cases.md` is
  paired with `pos_golden_cases.rs` (the oracle). Keep the flow and its test in step.
- `tests/features/**` and `docs/**` are `user_owned` paths — the generator never touches them.

## Review expectations

A reviewer checks five things, in order:

1. **Did the change start in the right place?** Schema for shape, `*_custom.rs` for logic.
2. **Regen-safety.** Nothing valuable sits where the next `generate --force` would eat it.
3. **Layer discipline.** Domain imports nothing transport/DB; arrows point inward.
4. **Consistency.** Terms match the [Glossary](08-glossary.md); the twelve CRUD endpoints are not
   re-implemented by hand.
5. **Proof.** Tests exist and pass; migrations are reversible.

Expect a request to move logic into a protected region if it is in generated territory — that is
the most common round-trip, and it is not a nit.

## Architectural changes

If your change is a *decision* (a new dependency, a new layer, a convention shift), write an ADR.
This module already carries [`docs/adr/ADR-001-pos-boundary-and-retail-seam.md`](../adr/ADR-001-pos-boundary-and-retail-seam.md)
— the POS boundary + retail-seam decision (POS posts no GL; it orchestrates billing and payment
through ports). Use it as the model for a real module ADR; number new module decisions onward
(ADR-002…). The three framework-wide ADRs live under [`handbook/adr/`](adr/) (see the
[template](adr/adr-0001-schema-yaml-ssot.md) for the shape). ADRs are immutable once accepted;
supersede rather than edit.

---

Related: [Glossary](08-glossary.md) · [Maintainer Guide](05-maintainer-guide.md) · [ADRs](adr/).
