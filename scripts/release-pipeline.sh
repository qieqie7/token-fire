#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: scripts/release-pipeline.sh --bundle <app|dmg> <--allow-dirty|--clean-required>" >&2
}

fail() {
  echo "error: $*" >&2
  exit 1
}

check_dmg_artifact() {
  local dmg_dir="src-tauri/target/release/bundle/dmg"
  local dmgs=()

  [ -d "${dmg_dir}" ] || fail "missing DMG output directory: ${dmg_dir}"

  while IFS= read -r dmg; do
    dmgs+=("${dmg}")
  done < <(find "${dmg_dir}" -maxdepth 1 -type f -name "*.dmg" | sort)

  if [[ "${#dmgs[@]}" -eq 0 ]]; then
    fail "no DMG found in ${dmg_dir}; check Tauri bundle targets"
  fi

  if [[ "${#dmgs[@]}" -gt 1 ]]; then
    printf '%s\n' "${dmgs[@]}" >&2
    fail "found multiple DMGs in ${dmg_dir}; clean the directory and rerun"
  fi
}

bundle=""
dirty_policy=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --bundle)
      [ "$#" -ge 2 ] || {
        usage
        fail "--bundle requires app or dmg"
      }
      bundle="$2"
      shift 2
      ;;
    --allow-dirty)
      dirty_policy="allow"
      shift
      ;;
    --clean-required)
      dirty_policy="clean"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      fail "unknown argument: $1"
      ;;
  esac
done

[ "${bundle}" = "app" ] || [ "${bundle}" = "dmg" ] || {
  usage
  fail "--bundle must be app or dmg"
}

[ "${dirty_policy}" = "allow" ] || [ "${dirty_policy}" = "clean" ] || {
  usage
  fail "choose --allow-dirty or --clean-required"
}

version="$(node -e 'const fs = require("fs"); console.log(JSON.parse(fs.readFileSync("package.json", "utf8")).version)')"
eval "$(pnpm --silent release:build-identity-env)"
export TOKEN_FIRE_GIT_COMMIT TOKEN_FIRE_GIT_COMMIT_SHORT TOKEN_FIRE_GIT_DIRTY TOKEN_FIRE_BUILD_TIME

pnpm release:check-version

if [ "${dirty_policy}" = "clean" ] && [ "${TOKEN_FIRE_GIT_DIRTY}" = "true" ] && [ "${TOKEN_FIRE_ALLOW_DIRTY_RELEASE:-}" != "1" ]; then
  echo "release-pipeline refuses dirty builds; set TOKEN_FIRE_ALLOW_DIRTY_RELEASE=1 only for diagnostic builds" >&2
  exit 1
fi

host_triple="$(rustc -vV | sed -n 's/^host: //p')"
[ -n "${host_triple}" ] || fail "failed to read Rust host triple"

TAURI_CONFIG='{"bundle":{"externalBin":[]}}' cargo build --manifest-path src-tauri/Cargo.toml --bin token-fire-hook --release
mkdir -p src-tauri/bin
cp "src-tauri/target/release/token-fire-hook" "src-tauri/bin/token-fire-hook-${host_triple}"
chmod +x "src-tauri/bin/token-fire-hook-${host_triple}"

cargo test --manifest-path src-tauri/Cargo.toml
pnpm test
pnpm build

if [ "${bundle}" = "app" ]; then
  pnpm tauri build --bundles app
elif [ "${bundle}" = "dmg" ]; then
  pnpm tauri build
  check_dmg_artifact
fi

app_path="src-tauri/target/release/bundle/macos/TokenFire.app"
app_bin_path="${app_path}/Contents/MacOS/token-fire"
app_hook_path="${app_path}/Contents/MacOS/token-fire-hook"
app_tray_icon_path="${app_path}/Contents/Resources/icons/tray-icon.png"
identity_dir="$(mktemp -d)"

test -x "src-tauri/bin/token-fire-hook-${host_triple}"
test -d "${app_path}"
test -x "${app_bin_path}"
test -x "${app_hook_path}"
test -s "${app_tray_icon_path}"

"${app_bin_path}" --version-json > "${identity_dir}/app.json"
"${app_hook_path}" --version-json > "${identity_dir}/hook.json"

node scripts/check-build-identity-output.mjs \
  "${identity_dir}/app.json" \
  "${identity_dir}/hook.json" \
  "${version}" \
  "${TOKEN_FIRE_GIT_COMMIT}" \
  "${TOKEN_FIRE_GIT_COMMIT_SHORT}" \
  "${TOKEN_FIRE_GIT_DIRTY}"

rg -n "Traex|rollout|session_meta|turn_context|~/.trae|traecli.toml" src-tauri/src/core && exit 1 || true
