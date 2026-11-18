#!/bin/sh
# Build a release dsh-tui and install it under ~/.local/bin (override PREFIX).
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
REPO_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/dsh-tui-target}"
CARGO_CACHE="${CARGO_HOME:-$HOME/.cache/cargo}"
RUSTFLAGS_VAL="${RUSTFLAGS:--C target-cpu=znver2}"

mkdir -p "$BIN_DIR" "$TARGET_DIR"

if command -v cargo >/dev/null 2>&1; then
    cd "$REPO_DIR"
    RUSTFLAGS="$RUSTFLAGS_VAL" cargo build --release
    if [ -n "${CARGO_TARGET_DIR:-}" ] && [ -f "$CARGO_TARGET_DIR/release/dsh-tui" ]; then
        BIN_PATH="$CARGO_TARGET_DIR/release/dsh-tui"
    else
        BIN_PATH="$REPO_DIR/target/release/dsh-tui"
    fi
elif command -v podman >/dev/null 2>&1; then
    IMAGE="${DSH_TUI_BUILDER_IMAGE:-localhost/craft-builder:latest}"
    mkdir -p "$CARGO_CACHE/registry" "$CARGO_CACHE/git"
    podman run --rm \
        -v "$REPO_DIR:/work" \
        -v "$CARGO_CACHE/registry:/root/.cargo/registry" \
        -v "$CARGO_CACHE/git:/root/.cargo/git" \
        -v "$TARGET_DIR:/target" \
        -w /work \
        -e CARGO_TARGET_DIR=/target \
        -e RUSTFLAGS="$RUSTFLAGS_VAL" \
        "$IMAGE" \
        bash -c "cargo build --release --offline || cargo build --release"
    BIN_PATH="$TARGET_DIR/release/dsh-tui"
else
    printf 'dsh-tui: brak cargo oraz podman w PATH\n' >&2
    exit 1
fi

install -m 755 "$BIN_PATH" "$BIN_DIR/dsh-tui"
printf 'dsh-tui: zainstalowano w %s/dsh-tui\n' "$BIN_DIR"
