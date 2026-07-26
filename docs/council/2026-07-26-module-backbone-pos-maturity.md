<!-- 2026-07-26 | repo type: module | unit: backbone-pos | focus: maturity | roster: Steelman, DDD/Bounded-Context, Contract Seat, Domain Expert (retail POS), Skeptic, YAGNI/Business — adjudicated by Chair. Steelman/Skeptic/Chair ran as isolated subagents. -->

> **Orchestrator post-hoc verification (2026-07-26).** The Chair flagged v0.2.1's publish state as
> the one unverified fact. Confirmed **PUBLISHED** — `git ls-remote` shows `refs/tags/v0.2.1 → 3b7e907`
> (pushed earlier this session). Facts 1 (`serpa-posman` pin = `v0.1.5`), 2 (vendored clone at
> `serpa-workspace/modules/backbone-pos` = `0.1.5`), and the `[patch."https://github.com/faridlab/backbone-pos"]`
> → `../../modules/backbone-pos` override are all confirmed verbatim. **Net effect on the Best call:**
> step (a) "publish v0.2.1 if absent" is already done — the move is now purely consumer-side (bump pin
> `0.1.5 → 0.2.1`, refresh the vendored clone, decide the `[patch]`). The divergence the Skeptic/Chair
> found is real and is the spine of this run; only the publish sub-step is resolved, not the gap.

---

# Council — module:backbone-pos — focus: maturity

## Best call
Cut the v0.2.1 release and converge serpa-posman onto it as one coordinated move: (a) publish `v0.2.1` as a git tag if `git tag --list` shows it absent; (b) bump `apps/serpa-posman-service/Cargo.toml:38` from `tag = "v0.1.5"` to `tag = "v0.2.1"`; (c) `metaphor sync` to refresh the vendored clone at `serpa-workspace/modules/backbone-pos` from v0.1.5 to v0.2.1; (d) keep or remove the `[patch."https://github.com/faridlab/backbone-pos"]` override so workspace and tag resolution agree. This dominates under every deploy model: under monorepo deploy it converts an invisible double-lag (tag pin + vendored clone both v0.1.5) into a single source of truth; under tag deploy it is the *only* move that puts the fail-closed RLS fence, the returns-idempotency addendum, and the post-v0.1.5 recognize_sale fixes into an artifact that can run. The deploy mechanism does not branch the call — I own the side that says the move is correct regardless.

> *(Orchestrator note, above: step (a) is already satisfied — v0.2.1 is published. The move reduces to (b) + (c) + (d), all consumer-side in `serpa-posman-service`.)*

- Residual negative value: until convergence, every cited maturity property is unobtainable by the only production consumer on every build path — a release deploy today ships unfenced `pos_invoice_items`/`pos_payments` (the load-bearing ADR-0010 fail-closed guarantee is absent at v0.1.5), drops the returns-idempotency bus-satisfiability addendum, and drops post-v0.1.5 `recognize_sale` fixes. Cost to land: one pin bump, one `metaphor sync`, one `[patch]` decision — hours, not days (no tag-cut needed; v0.2.1 is published). Residual risk *after* landing: `apply_settlement` is a two-service write inside `PaymentPort::settle` with no compensation (parked, presupposes v0.2.1 running); concurrent `recognize_sale` retry raises the double-revenue path because persist-and-reuse closes only sequential retry (parked, same); the consumer now tracks v0.2.1's *implicit* surface (the DDD tree), so the next regen restructuring can break it.
- Reversibility: easy. A published tag cannot be unpublished, but a follow-up v0.2.2 is trivial; the pin bump, vendored-clone refresh, and [patch] decision are pure reverts.
- What would flip this: evidence that serpa-posman is *not* the production consumer (Steelman condition 1 false) — leverage drops, but the move still dominates because it converts an unverifiable maturity claim into a checkable one. Nothing else flips it; the Skeptic's spine and the verified double-lag make the move dominant under both deploy models.

## Disagreement map
The real tensions. For each: the crux, and who is on each side.
- **Maturity-is-shipped vs. maturity-is-trapped** — Steelman, Domain Expert, and Contract Seat implicitly cite seven maturity properties as live; Skeptic verifies the v0.2.1 artifact carrying them is unreachable by the consumer, and the Chair's verification strengthens this (the consumer's vendored clone is *also* v0.1.5, so even the in-workspace `[patch]` build runs v0.1.5). Crux: is the artifact under review the artifact that runs? Verified NO. Skeptic wins; this is the spine of the run.
- **Adoption gap vs. feature gap** — YAGNI/Business says the gap this month is adoption (cost of waiting on parked features ≈ 0; no merchant demand); Domain Expert says two daily retail rules cannot be honored (partial/line-level returns; GL drawer-variance posting). Crux: which blocks launch? Collapsed by the spine — neither matters while v0.2.1 is unreachable; YAGNI's adoption move *is* the precondition for Domain Expert's rules to matter at all.
- **Implicit deep-path contract vs. deliberate integration surface** — Contract Seat says the outward promise is implicit (consumer reaches into `application::service::pos_events::PosEventSink`, `PosWriteService`, `presentation::http::{create_guarded_pos_routes_with_outbox, TenantVerifier}`; no semver). Crux: is the restructuring risk acceptable *now*? Today yes — one consumer, regen can be coordinated; the moment v0.2.1 publishes and a second consumer appears, a deliberate `backbone_pos::integration::{...}` re-export becomes the higher-leverage move. Until then it is parked behind the convergence.

## Recommendations (ranked by leverage)
| # | Move | Leverage | Residual negative | Reversibility | Evidence to flip |
|---|------|----------|-------------------|---------------|------------------|
| 1 | Converge serpa-posman onto the published v0.2.1 (bump pin `0.1.5→0.2.1`, `metaphor sync` the vendored clone, decide the `[patch]`) | Converts an unverifiable maturity claim into a checkable, runnable artifact; closes the silent RLS-fence drop on every release deploy | Hours; consumer now tracks v0.2.1's implicit DDD-tree surface | Easy (revert pin/sync/patch; cut v0.2.2) | serpa-posman is not the production consumer (drops leverage, not correctness) |
| 2 | Land real adapters + recognition subscription + outbox relay + enable RLS in serpa-posman | Without this, v0.2.1's sophistication runs nowhere — proven only against test adapters (`tests/retail_sale_seam.rs:48-179`, synchronous line 254) | Days of wiring; `apply_settlement`-no-compensation still parked | Costly (real adapter code) | A second consumer already exercises these paths in prod |
| 3 | If #1 blocked: probe with `git tag --list` + clean-tag `cargo tree -i backbone-pos` | Confirms publish state and the tag-resolution path independently *(already done: v0.2.1 is published; resolution lands on v0.1.5 only via the pin/clone)* | Hours of delay; no negative beyond the blocker | Trivial | `git tag --list` shows v0.2.1 already published under another ref ✓ |
| 4 | Add deliberate `backbone_pos::integration::{...}` stable surface | Protects v0.2.1 from regen restructuring silently breaking the consumer | Small re-export work; locks a surface before a second consumer informs it | Easy to widen, hard to narrow | A second consumer appears wanting a different shape |
| 5 | Optional line-subset → credit_note + proportional refund on `return_sale` | Cashiers can process the most common real return (currently impossible — `return_sale` takes a ticket, not a line subset) | Days of schema/service/migration work; touches ADR-001 park | Costly (migration) | Launch market confirms cash refund tickets cover all return cases |

## Maturity scorecard
Score each seated technical seat on ITS OWN axis.
| Seat | Axis | Score (1–5) | One sentence why |
|------|------|-------------|------------------|
| DDD / Bounded-Context | bounded context language consistent and contracts to siblings stable under change | 4 | One clean context with billing/payment vocabulary confined to the port ACL; docked one point because `PosProfile` absorbs accounting's GL chart-of-accounts language (income/receivable/cash/tax/cogs/inventory account ids) — defensible as the orchestrator's price, but a real leak. |
| Contract Seat | outward contract explicit and minimal, internals free to change, consumers depend only on deliberate promises | 2 | No deliberate surface; serpa-posman reaches into `application::service::pos_events::PosEventSink`, `PosWriteService`, `presentation::http::{create_guarded_pos_routes_with_outbox, TenantVerifier}`; the DDD tree is the de-facto contract with no semver, and the v0.1.5/v0.2.1 divergence is reachable by neither tag nor vendored-clone resolution. |
| Domain Expert | ubiquitous language consistent end-to-end and model represents every real business state/rule incl. edge cases | 3 | Core sale/multi-tender/drawer is faithful and Indonesia-correct (IDR math, server-side PPN 11%); docked two points because two daily retail rules — partial/line-level returns and GL drawer-variance posting — cannot be honored, and suspended/hold-ticket state may be missing (to verify). |

## Parking lot
Ideas raised but out of this run's focus — captured for a later council, not acted on now.
- Partial / line-level returns on `return_sale` — raised by Domain Expert, scope: this module's return path (ADR-001 park).
- GL drawer-variance posting (POS computes + persists + emits, posts no GL, no consumer posts either) — raised by Domain Expert, scope: cross-module POS → accounting.
- `apply_settlement` two-service write inside `PaymentPort::settle` with no compensation — raised by Steelman (condition 6) / Skeptic (second-order), scope: cross-module POS ↔ payment-gateway.
- Concurrent `recognize_sale` retry double-raise (persist-and-reuse closes sequential retry only) — raised by Steelman (condition 3) / Skeptic, scope: this module's recognize_sale path.
- Suspended / hold-ticket state for park-and-resume — raised by Domain Expert (verify), scope: this module's sale lifecycle.
- Deliberate `backbone_pos::integration::{...}` stable re-export surface — raised by Contract Seat, scope: this module's `lib.rs` / public API.
- Multi-currency / non-IDR / non-Indonesia tax — implicit in Steelman condition 4, scope: this module's money/tax primitives.
