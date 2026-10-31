#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
cargo zigbuild --locked --release --features pocketbook --target armv7-unknown-linux-gnueabi.2.19
python3 scripts/package-pocketbook.py "$CARGO_TARGET_DIR/armv7-unknown-linux-gnueabi/release/paper-calendar"
