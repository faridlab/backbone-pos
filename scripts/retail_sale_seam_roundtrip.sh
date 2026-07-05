#!/usr/bin/env bash
# Extension-contract §5 for the POS retail-sale seam: prove the cross-module port/ACL wiring survives a
# regeneration of the modules it spans. Snapshots the seam files, regenerates pos + billing + payment
# with --force, asserts byte-identical, and re-runs the end-to-end seam test green.
# Usage: DATABASE_URL=... bash scripts/retail_sale_seam_roundtrip.sh
set -euo pipefail
cd "$(dirname "$0")/.."

POS_FILES=(
  src/application/service/pos_write_service.rs
  src/application/service/pos_events.rs
  src/application/service/pos_ports.rs
  src/presentation/http/guarded_routes.rs
  tests/retail_sale_seam.rs
)
DOWN_FILES=(
  ../backbone-billing/src/application/service/billing_write_service.rs
  ../backbone-payment/src/application/service/payment_write_service.rs
)

echo "→ snapshot seam port/ACL files (pos + the emitters it drives)"
before=$(shasum -a 256 "${POS_FILES[@]}" "${DOWN_FILES[@]}")

echo "→ regenerate the modules the seam spans (§5) — billing, payment, then pos"
( cd ../backbone-billing && metaphor schema schema generate --force >/dev/null )
( cd ../backbone-payment && metaphor schema schema generate --force >/dev/null )
metaphor schema schema generate --force >/dev/null

echo "→ verify every seam file is byte-identical after regen"
after=$(shasum -a 256 "${POS_FILES[@]}" "${DOWN_FILES[@]}")
if [ "$before" != "$after" ]; then
  echo "✗ FAIL: a seam file changed during regen"; diff <(echo "$before") <(echo "$after") || true; exit 1
fi
echo "  ✓ all ${#POS_FILES[@]}+${#DOWN_FILES[@]} seam files unchanged"

echo "→ re-run the end-to-end retail-sale seam post-regen"
cargo test --test retail_sale_seam -- --test-threads=1 >/dev/null
echo "  ✓ POS→billing→payment→accounting seam still green after regenerating all three modules"
echo "✓ §5 round-trip proven for the retail-sale seam."
