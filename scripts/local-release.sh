#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "错误：$*" >&2
  exit 1
}

require_file() {
  local path="$1"
  [[ -f "$path" ]] || fail "缺少必要文件：$path"
}

require_dir() {
  local path="$1"
  [[ -d "$path" ]] || fail "缺少必要目录：$path"
}

echo "==> 检查仓库结构"
require_file "package.json"
require_file "src-tauri/tauri.conf.json"
require_file "src-tauri/Cargo.toml"
require_file ".gitignore"

echo "==> 检查 git 状态"
if [[ -n "$(git status --porcelain)" ]]; then
  git status --short
  fail "git worktree 不干净；发布前请先提交或暂存变更"
fi

release_dir="dist-app"

echo "==> 检查 dist-app 忽略规则"
git check-ignore -q "${release_dir}/" || fail "生成发布资产前，${release_dir}/ 必须已被 .gitignore 忽略"

echo "==> 检查版本一致性"
pnpm release:check-version

version="$(node -e 'const fs = require("fs"); console.log(JSON.parse(fs.readFileSync("package.json", "utf8")).version)')"

echo "==> 运行完整 release pipeline"
scripts/release-pipeline.sh --bundle dmg --clean-required

dmg_dir="src-tauri/target/release/bundle/dmg"

echo "==> 查找 DMG"
require_dir "$dmg_dir"
dmgs=()
while IFS= read -r dmg; do
  dmgs+=("$dmg")
done < <(find "$dmg_dir" -maxdepth 1 -type f -name "*.dmg" | sort)

if [[ "${#dmgs[@]}" -eq 0 ]]; then
  fail "未在 ${dmg_dir} 找到 DMG；请确认 pnpm tauri build 已完成 dmg target"
fi

if [[ "${#dmgs[@]}" -gt 1 ]]; then
  printf '%s\n' "${dmgs[@]}" >&2
  fail "找到多个 DMG；请清理 ${dmg_dir} 后重跑"
fi

source_dmg="${dmgs[0]}"
source_dmg_name="$(basename "$source_dmg")"
target_dmg="${release_dir}/${source_dmg_name}"
sha_file="${target_dmg}.sha256"
notes_file="${release_dir}/release-notes-v${version}.md"

echo "==> 清理 dist-app 发布目录"
rm -rf "$release_dir"
mkdir -p "$release_dir"

echo "==> 复制发布资产"
cp "$source_dmg" "$target_dmg"

echo "==> 写入 SHA-256"
(
  cd "$release_dir"
  shasum -a 256 "$source_dmg_name" > "${source_dmg_name}.sha256"
)
sha256_value="$(cut -d ' ' -f 1 "$sha_file")"

echo "==> 写入发布说明草稿"
cat > "$notes_file" <<NOTES
# TokenFire v${version}

## 下载

上传以下文件到本次 GitHub Release：

- \`${source_dmg_name}\`
- \`${source_dmg_name}.sha256\`

## 安装

打开 DMG，将 TokenFire 拖到 Applications。

## 免费分发说明

当前构建未使用 Developer ID 签名，也未经过 Apple notarization。macOS 可能阻止首次打开。

如果 macOS 阻止打开，可以先尝试右键点击 TokenFire，选择“打开”，或在系统设置中允许打开。

如仍无法打开，可执行：

\`\`\`bash
xattr -dr com.apple.quarantine /Applications/TokenFire.app
\`\`\`

## 校验

SHA-256:

\`\`\`text
${sha256_value}  ${source_dmg_name}
\`\`\`
NOTES

echo
echo "发布资产已生成："
echo "  ${target_dmg}"
echo "  ${sha_file}"
echo "  ${notes_file}"
echo
echo "SHA-256:"
echo "  ${sha256_value}  ${source_dmg_name}"
echo
echo "手动发布步骤："
echo "  1. 创建或编辑 v${version} 对应的 GitHub Release。"
echo "  2. 上传 ${target_dmg} 和 ${sha_file}。"
echo "  3. 复制 ${notes_file} 的内容作为发布说明。"
echo
echo "免费分发提示："
echo "  当前构建未使用 Developer ID 签名，也未经过 Apple notarization。"
echo "  接收方可能需要右键打开，或移除 com.apple.quarantine。"
