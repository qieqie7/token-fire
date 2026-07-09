# 仓库指南

## 项目结构与模块组织

TokenFire 是 macOS Tauri v2 小组件，前端使用 React，后端使用 Rust。前端代码在 `src/`：`src/app/` 管理应用 hooks 与 Tauri 窗口行为，`src/widget/` 放可复用小组件 UI，`src/design-system/` 放 CSS tokens，`src/style.css` 放应用级样式。Rust 代码在 `src-tauri/src/`：`core/` 是与来源无关的领域逻辑，`adapters/` 集成 Traex/Codex 来源，`app/` 编排 runtime/tray/widget state，`bin/token_fire_hook.rs` 构建 hook sidecar。Rust 集成测试在 `src-tauri/tests/`；fixtures 在 `src-tauri/tests/fixtures/`。计划和规格文档在 `docs/superpowers/`；长期设计说明在 `agent-docs/`。

## 构建、测试与开发命令

- `pnpm dev`: 只运行 Vite 前端。
- `pnpm tauri dev`: 运行真实桌面应用；用于验证原生拖拽、透明窗口、`alwaysOnTop` 和固定窗口尺寸。
- `pnpm build`: 先用 `tsc` 做 TypeScript 类型检查，再构建 Vite 产物。
- `pnpm test`: 运行匹配 `src/**/*.test.ts(x)` 的 Vitest 测试。
- `cargo test --manifest-path src-tauri/Cargo.toml`: 运行 Rust 单元测试与集成测试。
- `scripts/app-bundle-smoke.sh`: 构建 hook sidecar，运行检查，并创建 macOS `.app` bundle。
- `scripts/release-smoke.sh`: 执行完整 release smoke，包含 DMG 打包和 core 边界字符串检查。

## 编码风格与命名约定

使用 TypeScript `strict` 模式、React JSX 和普通 CSS variables。不要引入 Tailwind 或 styled-components。设计文档中的 tokens 使用 dot.case，CSS custom properties 使用 kebab-case。Rust 使用 edition 2021 和标准 `cargo fmt` 风格。保持 core/adapter 边界：`src-tauri/src/core/` 不应依赖 Traex、Codex、文件系统布局或应用 runtime 细节。

## 测试指南

前端测试放在相关代码旁，命名为 `*.test.ts` 或 `*.test.tsx`。Rust 测试放在 `src-tauri/tests/`，覆盖 runtime、storage、adapters、hook config 和 command 行为。Vitest 扫描必须排除 `.worktrees/`。Tauri UI 行为不能只用 browser/Vite preview 验证；需要用 `pnpm tauri dev` 或 bundle smoke 脚本验证。

## 提交与 Pull Request 规范

近期历史使用 Conventional Commit 前缀，例如 `feat:`、`fix:` 和 `chore:`。提交应保持范围清晰、描述具体，例如 `fix: guard widget state listener lifecycle`。PR 需要说明用户可见变更、测试证据；如果修改 UI/窗口行为，需要写明原生验证结果；视觉小组件更新需要附截图。sidecar 或 bundle 变更必须显式说明。

## Agent 专用说明

直接使用 `pnpm`，不要加 `corepack`。shell 示例必须能从仓库根目录直接复制执行。不要把 `dist/`、`src-tauri/target/`、`.worktrees/` 或 `node_modules/` 里的生成产物加入提交。
