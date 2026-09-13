#!/usr/bin/env bash
# Builds a release bundle of the RePlayrer desktop app (Tauri) and
# packages it as a tar.gz (AppImage + a sample scenario) for manual
# testing on another Linux machine.
#
# Usage: scripts/package.sh
#
# Requires:
#   - cargo/rustup on PATH
#   - the `tauri` cargo subcommand, matching the `tauri = "2.11.3"`
#     pinned in src-tauri/Cargo.toml:
#       cargo install tauri-cli --locked --version "^2"
#   - Node/npm on PATH (tauri.conf.json's beforeBuildCommand runs
#     `npm run build --prefix frontend`)
#   - the native libs Tauri needs to build a Linux bundle — on
#     Debian/Ubuntu:
#       apt install libwebkit2gtk-4.1-dev libappindicator3-dev \
#         librsvg2-dev patchelf
#     see https://tauri.app/start/prerequisites/ for other distros.
#
# Linux x86_64 only: cross-building a full GUI Tauri bundle for
# Windows/macOS from Linux isn't the same easy mingw cross-compile
# backend/scripts/package.sh does for the CLI (Windows needs WebView2 +
# Windows-specific bundler tooling; macOS needs Xcode) — build those on
# a machine actually running that OS.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_TAURI_DIR="$ROOT_DIR/src-tauri"
BACKEND_DIR="$ROOT_DIR/backend"
DIST_DIR="$ROOT_DIR/dist"
PACKAGING_DIR="$ROOT_DIR/packaging"
VERSION="$(grep -m1 '^version' "$SRC_TAURI_DIR/Cargo.toml" | cut -d '"' -f2)"

log() { echo "==> $*"; }

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found on PATH" >&2
    exit 1
fi
if ! cargo tauri --version >/dev/null 2>&1; then
    echo "error: the 'tauri' cargo subcommand isn't installed." >&2
    echo "       install it with: cargo install tauri-cli --locked --version \"^2\"" >&2
    exit 1
fi

mkdir -p "$DIST_DIR"

STAGE="$DIST_DIR/.stage-linux-x86_64"
ARCHIVE="$DIST_DIR/replayrer-${VERSION}-linux-x86_64.tar.gz"
rm -rf "$STAGE"
mkdir -p "$STAGE"

log "generating sample.pcap"
( cd "$BACKEND_DIR" && cargo run --quiet --release --package format-pcap \
    --example generate_sample -- "$STAGE/sample.pcap" >/dev/null )

cat >"$STAGE/scenario.toml" <<'EOF'
[[recordings]]
id = "sample"
format = "pcap"
paths = ["sample.pcap"]

[[outputs]]
id = "udp-out"
kind = "udp"
address = "127.0.0.1:5000"

[[routes]]
recording_id = "sample"
output_id = "udp-out"
EOF

log "building release bundle (also builds the frontend via beforeBuildCommand)"
# linuxdeploy/appimagetool are themselves AppImages that tauri-bundler
# downloads and runs — on a VM/container without FUSE (the common case),
# that fails with exactly "failed to run linuxdeploy" and no further
# detail. APPIMAGE_EXTRACT_AND_RUN=1 tells them to extract-and-run
# instead of FUSE-mounting themselves.
( cd "$SRC_TAURI_DIR" && NO_STRIP=true APPIMAGE_EXTRACT_AND_RUN=1 cargo tauri build --bundles appimage )

APPIMAGE="$(find "$SRC_TAURI_DIR/target/release/bundle/appimage" -maxdepth 1 -iname "*.AppImage" | head -1)"
if [ -z "$APPIMAGE" ]; then
    echo "error: no .AppImage found under src-tauri/target/release/bundle/appimage" >&2
    exit 1
fi

cp "$APPIMAGE" "$STAGE/replayrer.AppImage"
chmod +x "$STAGE/replayrer.AppImage"
cp "$PACKAGING_DIR/README-linux.txt" "$STAGE/README.txt"

tar -czf "$ARCHIVE" -C "$STAGE" .
rm -rf "$STAGE"
log "wrote $ARCHIVE"
