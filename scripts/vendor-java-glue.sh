#!/usr/bin/env bash
# Vendors miniquad's Android Java glue into android/java/.
#
# ADR-0005: we own this glue because cargo-apk and xbuild cannot package a
# miniquad app. Nothing version-checks these files against the miniquad crate,
# so the tag below MUST be bumped in lockstep with the miniquad version in
# Cargo.lock, or the app breaks at runtime with no build error.
#
# macroquad 0.4.16 pins miniquad = "=0.4.11" exactly, so this tag tracks that.
set -euo pipefail

MINIQUAD_TAG="0.4.11"
BASE="https://raw.githubusercontent.com/not-fl3/miniquad/${MINIQUAD_TAG}/java"

cd "$(dirname "$0")/.."
DEST="android/java/quad_native"
mkdir -p "$DEST"

for f in MainActivity.java QuadNative.java QuadSurface.java; do
  echo "fetching $f from miniquad ${MINIQUAD_TAG}"
  curl -fsSL "$BASE/$f" -o "$DEST/$f"
done

sha256sum "$DEST"/*.java > "$DEST/SHA256SUMS"
echo
echo "Vendored from miniquad ${MINIQUAD_TAG}. Checksums written to $DEST/SHA256SUMS."
echo "Verify with: (cd $DEST && sha256sum -c SHA256SUMS)"
