#!/usr/bin/env bash
set -euo pipefail

host_triple="$(rustc -vV | sed -n 's/^host: //p')"
TAURI_CONFIG='{"bundle":{"externalBin":[]}}' cargo build --manifest-path src-tauri/Cargo.toml --bin token-fire-hook --release
mkdir -p src-tauri/bin
cp "src-tauri/target/release/token-fire-hook" "src-tauri/bin/token-fire-hook-${host_triple}"
chmod +x "src-tauri/bin/token-fire-hook-${host_triple}"

cargo test --manifest-path src-tauri/Cargo.toml
pnpm test
pnpm build
pnpm tauri build
test -x "src-tauri/bin/token-fire-hook-${host_triple}"
test -x "src-tauri/target/release/bundle/macos/TokenFire.app/Contents/MacOS/token-fire-hook"
rg -n "Traex|rollout|session_meta|turn_context|~/.trae|traecli.toml" src-tauri/src/core && exit 1 || true
