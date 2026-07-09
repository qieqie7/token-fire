# TokenFire 发布版本管理

Last reviewed: 2026-07-08

## 目的

TokenFire release 必须能从 app UI 和复制来的日志里识别。原因是用户可能运行安装在 `/Applications/TokenFire.app` 的旧版本，而 repo 里已经有新代码；如果只替换了一个二进制，app runtime 和 hook sidecar 也可能漂移。

## 当前基线

当前 baseline release version 是 `0.1.0`。

已确认的 metadata files：

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.lock` 中 root package `token-fire`

这些文件必须使用同一个 release version。

## 版本模型

TokenFire release version 使用 SemVer：

- Patch：bug fix 和支持性修复，例如 `0.1.0 -> 0.1.1`。
- Minor：用户可见功能新增，例如 `0.1.1 -> 0.2.0`。
- Major：`1.0.0` 后的稳定兼容性破坏。

不要为每次开发提交 bump version。准备给别人安装或测试 app bundle / DMG 时才 bump version。

## Build Identity

Release version 和 build identity 是两个不同事实。

Release version 回答：

- 这是哪个命名测试包或发布包？

Build identity 回答：

- 这个二进制由哪份精确代码产生？

Build identity 应包含：

- `version`
- `git_commit`
- `git_commit_short`
- `build_time`
- `dirty`

`token-fire` 和 `token-fire-hook` 都必须暴露或记录 build identity。Hook 日志可能来自安装包里的 sidecar，即使 app runtime 或本地 repo 已经是另一份代码。

Release / smoke build 必须由脚本显式注入：

- `TOKEN_FIRE_GIT_COMMIT`
- `TOKEN_FIRE_GIT_COMMIT_SHORT`
- `TOKEN_FIRE_GIT_DIRTY`
- `TOKEN_FIRE_BUILD_TIME`

`rtk pnpm release:build-identity-env` 会输出 shell-safe assignments：

```sh
TOKEN_FIRE_GIT_COMMIT=...
TOKEN_FIRE_GIT_COMMIT_SHORT=...
TOKEN_FIRE_GIT_DIRTY=...
TOKEN_FIRE_BUILD_TIME=...
```

`scripts/app-bundle-smoke.sh` 和 `scripts/release-smoke.sh` 应消费这些 assignments，再把 build identity 注入 app runtime 和 hook sidecar build。

`build.rs` 可以为本地开发提供 git fallback，但对外分享包的证明链以脚本注入值为准。Smoke 必须检查 app / hook 的 `git_commit` 都等于当前 `HEAD`，且 app / hook 的 `version`、`git_commit`、`git_commit_short`、`dirty`、`build_time` 一致。

## UI 规则

紧凑 Profile header 可以在 `TokenFire` 品牌附近显示短版本标记：

```text
v0.1.1 · 7e17eb0
```

Profile 保持紧凑，不变成 release dashboard。完整 build details 放在日志和诊断包里。

## GitHub 更新提示

TokenFire 的更新提示以 `qieqie7/token-fire` 的 GitHub latest stable Release 为数据源。

应用启动后会静默检查一次，并在成功检查后的 24 小时内复用 `~/.token-fire/release-check.json` 缓存。发现远端版本高于当前 `build_identity.version` 时，Profile header 显示 `当前版本 · commitid 可更新`，点击后打开 GitHub Release 页面。

这不是自动更新器：不会自动下载 DMG，不会替换 `/Applications/TokenFire.app`，也不接入 `tauri-plugin-updater`。

## 日志规则

Runtime startup 应写入带 build identity 的 `app_started` 事件。

Hook sidecar events 应包含 build identity，尤其是：

- `hook_forwarded`
- `hook_socket_unavailable`
- hook input / parse failures

支持命令应该能用 `grep`、`tail`、`sqlite3` 检查这些日志；基础诊断不要依赖 `jq`。

## 发布流程

分享 app bundle 或 DMG 前按这个流程走：

1. 选择目标版本，例如 `0.1.1`。
2. 运行 `rtk pnpm release:bump -- <patch|minor|major|MAJOR.MINOR.PATCH>`，同时更新 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.lock`。
3. 确认 worktree 除了预期 release changes 外是 clean。
4. 运行 `rtk pnpm release:check-version`。
5. 运行 `rtk pnpm release:build-identity-env`，确认输出四行 `TOKEN_FIRE_*` assignments。
6. 运行 `scripts/app-bundle-smoke.sh`。
7. 如果是完整 release，运行 `scripts/release-smoke.sh`。
8. 确认 bundle 包含 `TokenFire.app/Contents/MacOS/token-fire`。
9. 运行 `rtk pnpm release:check-version`，并确认 logs 或 `--version-json` output 暴露目标 version 和 commit。
10. 确认 `token-fire --version-json` 和 `token-fire-hook --version-json` 都暴露目标 version、当前 `HEAD` commit、build time 和 dirty flag。

实现时必须让 version consistency 可机器校验。不要只依赖人工检查。

`scripts/app-bundle-smoke.sh` 和 `scripts/release-smoke.sh` 必须检查：

- `rtk pnpm release:check-version` 通过；
- app bundle 包含 `TokenFire.app/Contents/MacOS/token-fire`；
- app bundle 包含 `TokenFire.app/Contents/MacOS/token-fire-hook`；
- `token-fire --version-json` 的 `version` 等于 `package.json`；
- `token-fire-hook --version-json` 的 `version` 等于 `package.json`；
- app / hook 的 `git_commit` 都等于当前 `HEAD`；
- app / hook 的 `version`、`git_commit`、`git_commit_short`、`dirty`、`build_time` 必须一致；
- `scripts/release-smoke.sh` 默认拒绝 dirty build，除非 `TOKEN_FIRE_ALLOW_DIRTY_RELEASE=1`。

诊断包中的 `build_identity` 应使用 nested 结构：

```json
{
  "app_runtime": {},
  "hook_sidecar": {},
  "mismatch": false
}
```

`hook_sidecar` 优先来自实际 hook executable 的 `--version-json`。如果取不到，记录 `unavailable` 和 error kind。诊断包不得导出完整 `hook_path`；只允许 basename 或 install location category。
当前允许的 `hook_path` 类别是 `applications_bundle/token-fire-hook`、`dev_target/token-fire-hook` 和 `unknown/token-fire-hook`。

## Agent Guidance

修改 release、bundle、smoke、logging 或 Profile version display 行为时：

1. 保持所有 metadata files 的 version consistency。
2. 让 app runtime 和 hook sidecar 都能拿到 build identity。
3. 复制来的日志只有包含 version 和 full commit identity，才能说它证明了具体代码版本。
4. 把 dirty builds 当作 diagnostic builds，必须显式标记。
5. 对外 release 默认不能是 dirty build；临时分享 dirty build 必须明确标记为 diagnostic build。
6. Version diagnostics 不得包含 raw prompts、responses、transcript content、command output、raw hook payloads 或完整本地路径。
7. 如果 release workflow 变化，更新本文档。
