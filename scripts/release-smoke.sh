#!/usr/bin/env bash
set -euo pipefail

version="$(node -e 'const fs = require("fs"); console.log(JSON.parse(fs.readFileSync("package.json", "utf8")).version)')"
eval "$(pnpm --silent release:build-identity-env)"
export TOKEN_FIRE_GIT_COMMIT TOKEN_FIRE_GIT_COMMIT_SHORT TOKEN_FIRE_GIT_DIRTY TOKEN_FIRE_BUILD_TIME

pnpm release:check-version

if [ "${TOKEN_FIRE_GIT_DIRTY}" = "true" ] && [ "${TOKEN_FIRE_ALLOW_DIRTY_RELEASE:-}" != "1" ]; then
  echo "release-smoke refuses dirty builds; set TOKEN_FIRE_ALLOW_DIRTY_RELEASE=1 only for diagnostic builds" >&2
  exit 1
fi

host_triple="$(rustc -vV | sed -n 's/^host: //p')"
TAURI_CONFIG='{"bundle":{"externalBin":[]}}' cargo build --manifest-path src-tauri/Cargo.toml --bin token-fire-hook --release
mkdir -p src-tauri/bin
cp "src-tauri/target/release/token-fire-hook" "src-tauri/bin/token-fire-hook-${host_triple}"
chmod +x "src-tauri/bin/token-fire-hook-${host_triple}"

cargo test --manifest-path src-tauri/Cargo.toml
pnpm test
pnpm build
pnpm tauri build
app_bin="src-tauri/target/release/bundle/macos/TokenFire.app/Contents/MacOS/token-fire"
hook_bin="src-tauri/target/release/bundle/macos/TokenFire.app/Contents/MacOS/token-fire-hook"
identity_dir="$(mktemp -d)"

test -x "src-tauri/bin/token-fire-hook-${host_triple}"
test -x "${app_bin}"
test -x "${hook_bin}"

"${app_bin}" --version-json > "${identity_dir}/app.json"
"${hook_bin}" --version-json > "${identity_dir}/hook.json"

node scripts/check-build-identity-output.mjs \
  "${identity_dir}/app.json" \
  "${identity_dir}/hook.json" \
  "${version}" \
  "${TOKEN_FIRE_GIT_COMMIT}" \
  "${TOKEN_FIRE_GIT_COMMIT_SHORT}" \
  "${TOKEN_FIRE_GIT_DIRTY}"

rg -n "Traex|rollout|session_meta|turn_context|~/.trae|traecli.toml" src-tauri/src/core && exit 1 || true
