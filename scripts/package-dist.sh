#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="voicebox-asr"
DIST_DIR="$ROOT_DIR/dist/$APP_NAME"
MODEL_DIR_NAME="paraformer-zh-small-2024-03-09"
SOURCE_MODEL_DIR="$ROOT_DIR/models/$MODEL_DIR_NAME"
SOURCE_MODEL_FILE="$SOURCE_MODEL_DIR/model.int8.onnx"
SOURCE_TOKENS_FILE="$SOURCE_MODEL_DIR/tokens.txt"
RELEASE_BIN="$ROOT_DIR/target/release/$APP_NAME"

if [[ ! -f "$SOURCE_MODEL_FILE" ]]; then
  echo "Missing model file: $SOURCE_MODEL_FILE" >&2
  exit 1
fi

if [[ ! -f "$SOURCE_TOKENS_FILE" ]]; then
  echo "Missing tokens file: $SOURCE_TOKENS_FILE" >&2
  exit 1
fi

echo "Building release binary..."
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR/models/$MODEL_DIR_NAME"

cp "$RELEASE_BIN" "$DIST_DIR/$APP_NAME"
cp "$SOURCE_MODEL_FILE" "$DIST_DIR/models/$MODEL_DIR_NAME/model.int8.onnx"
cp "$SOURCE_TOKENS_FILE" "$DIST_DIR/models/$MODEL_DIR_NAME/tokens.txt"
chmod +x "$DIST_DIR/$APP_NAME"

cat > "$DIST_DIR/README.txt" <<'EOF'
VoiceBox ASR release layout

Start:
  ./voicebox-asr

Default URL:
  http://127.0.0.1:8765/

This folder must keep the following relative layout:
  voicebox-asr
  models/paraformer-zh-small-2024-03-09/model.int8.onnx
  models/paraformer-zh-small-2024-03-09/tokens.txt
EOF

echo "Packaged release directory:"
echo "  $DIST_DIR"
find "$DIST_DIR" -maxdepth 3 \( -type f -o -type l \) | sort
