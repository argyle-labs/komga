#!/usr/bin/env bash
# Re-vendor Komga's OpenAPI spec into specs/komga.openapi.json with sorted keys,
# so a refresh diffs only real upstream changes.
#   KOMGA_URL=https://komga.example KOMGA_API_KEY=... scripts/refresh-spec.sh
# Then run `cargo test`: it fails if an operation in src/operations.rs left the spec.
set -euo pipefail

: "${KOMGA_URL:?set KOMGA_URL}"
: "${KOMGA_API_KEY:?set KOMGA_API_KEY}"
ROOT="$(git rev-parse --show-toplevel)"
TMP="$(mktemp)"
OUT="$(mktemp "$ROOT/specs/.komga.openapi.XXXXXX")"
trap 'rm -f "$TMP" "$OUT"' EXIT

curl -fsS -H @- "${KOMGA_URL%/}/v3/api-docs" -o "$TMP" <<<"X-API-Key: ${KOMGA_API_KEY}"
# The live server URL is a private host; the vendored spec names a placeholder.
jq -S '.servers = [{"url": "https://komga.example"}]' "$TMP" > "$OUT"
chmod 644 "$OUT"
mv "$OUT" "$ROOT/specs/komga.openapi.json"
jq -r '"komga \(.info.version) (OpenAPI \(.openapi))"' "$ROOT/specs/komga.openapi.json"
