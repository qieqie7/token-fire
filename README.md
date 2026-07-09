# TokenFire

TokenFire 是一个本地 macOS AI 使用量仪表。它以 Tauri 菜单栏应用运行，从 TraeX、Codex、Claude 和 Cursor 等来源采集 token 元数据，并展示最近一年的使用热力图、周期汇总、来源分布和模型分布。

![TokenFire Profile 截图](docs/assets/tokenfire-profile.png)

## 展示内容

- 最近 365 天的活动热力图，包含活跃天数、token 量和预估人民币成本。
- 今日、本周、本月、今年的周期筛选。
- 当前周期的预估成本和 token 总量。
- TraeX、Codex、Claude、Cursor 的来源归因。
- 按 token 使用量排序的模型归因。
- macOS 菜单栏中的本地 hook 状态和来源开关。

成本只用于本地感知和对比，不是账单事实来源。

## 架构

TokenFire 是 macOS Tauri v2 应用，前端使用 React，后端使用 Rust。

- `src/profile/` 渲染 Profile 弹窗、热力图、指标、来源分布和模型分布。
- `src-tauri/src/core/` 负责与来源无关的统计、定价、聚合和 SQLite 存储。
- `src-tauri/src/adapters/` 放 TraeX、Codex、Claude、Cursor 的来源适配逻辑。
- `src-tauri/src/app/` 编排 runtime paths、tray 行为、socket 转发、hook 管理和 Profile commands。
- `src-tauri/src/bin/token_fire_hook.rs` 构建外部工具 hook 使用的 sidecar。

运行时数据保存在本地 `~/.token-fire`，包括 `token-fire.sqlite`、日志、socket 文件、备份和 debug bundles。

## 开发

安装依赖：

```bash
pnpm install
```

只运行 Vite 前端：

```bash
pnpm dev
```

运行真实桌面应用：

```bash
pnpm tauri dev
```

构建前端：

```bash
pnpm build
```

运行前端测试：

```bash
pnpm test
```

运行 Rust 测试：

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

## 验证脚本

应用 bundle smoke 检查：

```bash
scripts/app-bundle-smoke.sh
```

完整 release smoke 检查：

```bash
scripts/release-smoke.sh
```

`scripts/app-bundle-smoke.sh` 只构建 `.app` bundle。`scripts/release-smoke.sh` 会执行完整 Tauri build，用于检查 release 构建链路。

## 本地发布

生成本地发布资产：

```bash
scripts/local-release.sh
```

脚本会运行测试和完整 Tauri build，并把需要手动上传到 GitHub Release 的关键产物统一放到仓库根目录 `dist-app/`：

```text
dist-app/
  <Tauri 生成的 DMG 文件名>.dmg
  <Tauri 生成的 DMG 文件名>.dmg.sha256
  release-notes-v<version>.md
```

上传 GitHub Release 时，使用 `dist-app/` 里的 DMG 和 `.sha256` 文件，并复制 `release-notes-v<version>.md` 的内容作为发布说明草稿。

## 免费分发限制

当前本地发布路线不使用 Apple Developer Program，不使用 `Developer ID Application` 签名，也不经过 Apple notarization。

这意味着接收方下载 DMG 后，macOS 仍可能阻止首次打开。如果被系统拦截，可以先尝试右键点击 TokenFire，选择“打开”，或在系统设置中允许打开。

如仍无法打开，可执行：

```bash
xattr -dr com.apple.quarantine /Applications/TokenFire.app
```

这条路线适合自用、技术朋友和小范围内测。面向普通用户的顺滑安装体验需要另行接入 Developer ID 签名和公证。

## 仓库结构

```text
src/
  app/              React hooks 和 Tauri command bridge
  profile/          菜单栏 Profile UI
  design-system/    CSS tokens
src-tauri/
  src/core/         领域逻辑和存储
  src/adapters/     来源适配和 ingestion
  src/app/          runtime 编排
  src/bin/          hook sidecar
  tests/            Rust 集成测试
scripts/            构建、验证和本地发布脚本
docs/               规格文档和 README 资源
agent-docs/         长期设计说明
```

## 说明

- 前端使用 React、TypeScript 和普通 CSS variables。
- 后端使用 Rust 2021、Tauri v2 和 `rusqlite`。
- `dist/`、`dist-app/`、`src-tauri/target/`、`src-tauri/bin/`、`.worktrees/`、`node_modules/` 等生成产物不要加入 git。
