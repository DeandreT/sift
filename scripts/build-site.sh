#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEMO_DIR="$ROOT_DIR/crates/web-demo"
OUTPUT_DIR="$ROOT_DIR/out"

command -v trunk >/dev/null 2>&1 || {
  echo "trunk is required: https://trunkrs.dev/" >&2
  exit 1
}

cargo metadata --locked --format-version 1 --no-deps \
  --manifest-path "$ROOT_DIR/Cargo.toml" >/dev/null

(
  cd "$DEMO_DIR"
  trunk build --release --public-url ./
)

(
  cd "$ROOT_DIR"
  npm run build
)

mkdir -p "$OUTPUT_DIR/app"
cp -R "$DEMO_DIR/dist/." "$OUTPUT_DIR/app/"

# Trunk 0.16 can render a relative public URL as /./, which breaks Pages paths.
sed -i 's|"/\./|"./|g; s|'"'"'/\./|'"'"'./|g' "$OUTPUT_DIR/app/index.html"

echo "Site assembled in $OUTPUT_DIR"
