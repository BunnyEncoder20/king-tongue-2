#!/usr/bin/env bash
# Mechanically enforces ADR-0001: the Core Sim depends on glam and nothing else
# in its default feature set. This is the whole reason the boundary is real
# rather than cosmetic, so it runs in CI and should run in a pre-commit hook.
#
# Deliberately checks the DEFAULT feature set. `serde` is an opt-in feature and
# the Shell enables it; that is permitted by ADR-0001, which allows a parser
# behind a feature flag.
set -euo pipefail

cd "$(dirname "$0")/.."

EXPECTED="scripts/sim-deps.expected"
ACTUAL="$(mktemp)"
trap 'rm -f "$ACTUAL"' EXIT

cargo tree \
  --package king_tongue_sim \
  --no-default-features \
  --edges normal \
  --prefix none \
  --no-dedupe \
  2>/dev/null | sed 's/ (\*)$//' | sort -u > "$ACTUAL"

if ! diff -u "$EXPECTED" "$ACTUAL"; then
  echo >&2
  echo "FAIL: king_tongue_sim's default dependency set changed." >&2
  echo >&2
  echo "The Core Sim must depend on glam and nothing else (ADR-0001). If this is" >&2
  echo "a deliberate architectural change, update $EXPECTED and amend the ADR." >&2
  echo "If you need a parser or codec, put it behind an optional feature or move" >&2
  echo "it into the Shell." >&2
  exit 1
fi

echo "OK: king_tongue_sim default deps match ADR-0001 (glam only)."
