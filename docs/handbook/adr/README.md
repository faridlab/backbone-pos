# Architecture Decision Records

One decision per record: context, decision, alternatives, consequences. **Immutable once
accepted** — to change a decision, write a new ADR that supersedes the old one and update its
Status line; never edit an accepted decision in place.

## Framework decisions (inherited from the module skeleton)

These three explain how *any* Backbone module works. They are the model for a new decision's shape.

| ADR | Decision | Status |
|-----|----------|--------|
| [0001](adr-0001-schema-yaml-ssot.md) | Schema YAML is the single source of truth | Accepted |
| [0002](adr-0002-generic-crud.md) | CRUD is inherited from generics, not written per entity | Accepted |
| [0003](adr-0003-custom-markers.md) | Regen-safety via CUSTOM markers and `user_owned` | Accepted |

## Module decisions (POS-specific)

The decisions unique to `backbone-pos` live under [`docs/adr/`](../../adr/), numbered from `ADR-001`.

| ADR | Decision | Status |
|-----|----------|--------|
| [ADR-001](../../adr/ADR-001-pos-boundary-and-retail-seam.md) | POS owns the session + ticket; it posts no GL, orchestrating billing + payment (the retail seam) | Accepted |
