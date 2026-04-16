#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="voicebox-asr"
DIST_DIR="$ROOT_DIR/dist/$APP_NAME"
SOURCE_MODELS_DIR="$ROOT_DIR/models"
RELEASE_BIN="$ROOT_DIR/target/release/$APP_NAME"

if [[ ! -d "$SOURCE_MODELS_DIR" ]]; then
  echo "Missing models directory: $SOURCE_MODELS_DIR" >&2
  exit 1
fi

echo "Building release binary..."
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

cp "$RELEASE_BIN" "$DIST_DIR/$APP_NAME"
cp -R "$SOURCE_MODELS_DIR" "$DIST_DIR/models"
find "$DIST_DIR/models" -name '.DS_Store' -delete
chmod +x "$DIST_DIR/$APP_NAME"

cat > "$DIST_DIR/README.txt" <<'EOF'
VoiceBox ASR release layout

Start:
  ./voicebox-asr

Default URL:
  http://127.0.0.1:8765/

This folder must keep the following relative layout:
  voicebox-asr
  models/
EOF

echo "Packaged release directory:"
echo "  $DIST_DIR"
find "$DIST_DIR" -maxdepth 3 \( -type f -o -type l \) | sort
