---
name: release-version-build
description: 当准备发布版本升级，并且需要打 tag、推送、完成构建验证时使用
---

# 发布版本升级与构建

## 概览

按发布顺序执行：确认目标版本，升级版本元数据，验证一致性，提交，打 tag，推送，然后完成完整发布构建。

如果用户没有提供目标版本，修改文件前必须停止。根据 SemVer 语义给出建议版本，并等待用户确认。

## 何时使用

当用户提出类似需求时使用：

- "升级至 0.1.1 版本，打 tag，推送，再构建"
- "bump to x.x.x, tag, push, build"
- "发一个版本"
- "release version and build"

不要用于普通本地开发构建、单纯 smoke check，或没有发布意图的版本解释。

## 必须顺序

1. 确认目标版本。
2. 检查 worktree 和当前版本。
3. 运行仓库自带版本升级命令，或一致更新全部 release metadata。
4. 运行版本一致性检查。
5. 提交版本升级。
6. 为目标版本创建 annotated git tag。
7. 推送 commit 和 tag。
8. 完成要求的完整发布构建。
9. 汇报准确的 commit、tag、push、build 证据。

版本一致性检查通过前不要打 tag。构建成功前不要宣称完成。

## 未提供版本时

如果用户要求发版但没有提供版本：

1. 检查当前版本 metadata。
2. 选择最小合理 SemVer 升级：
   - patch：修复、打包、文档、发布流程修正
   - minor：用户可见功能新增
   - major：`1.0.0` 后明确破坏兼容性
3. 请求用户确认建议版本。
4. 用户回复前停止执行。

示例确认：

```text
当前版本是 0.1.0。我建议发布 0.1.1，因为这是 patch 级发布。确认后我会 bump、commit、tag、push，并完成构建。
```

## TokenFire 命令

从仓库根目录执行：

```bash
rtk git status --short
rtk pnpm release:check-version
rtk pnpm release:bump -- 0.1.1
rtk pnpm release:check-version
rtk git add package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json src-tauri/Cargo.lock agent-docs/release-versioning.md
rtk git commit -m "chore: release 0.1.1"
rtk git tag -a v0.1.1 -m "v0.1.1"
rtk git push
rtk git push origin v0.1.1
rtk bash scripts/local-release.sh
```

替换示例里的版本号。只有 baseline release 文案需要同步时，才把 `agent-docs/release-versioning.md` 加入提交。

`scripts/release-pipeline.sh --bundle app --allow-dirty` 只验证 `.app` bundle，不产出 DMG。`scripts/release-pipeline.sh --bundle dmg --clean-required` 产出 Tauri 原始 DMG 到 `src-tauri/target/release/bundle/dmg/`，但不会生成发布用的 `dist-app/` 汇总目录。

用户要求“打包的 DMG 文件”或完整发布构建时，必须使用 `scripts/local-release.sh`。它会调用完整 DMG pipeline，并生成：

- `dist-app/TokenFire_X.Y.Z_aarch64.dmg`
- `dist-app/TokenFire_X.Y.Z_aarch64.dmg.sha256`
- `dist-app/release-notes-vX.Y.Z.md`

如果只运行了 `scripts/release-pipeline.sh --bundle dmg --clean-required`，还没有完成可上传发布资产整理；需要补齐 `dist-app`，或改跑 `scripts/local-release.sh`。

如果 `git tag -a vX.Y.Z` 失败并提示 tag already exists：

1. 停止发布流程。
2. 检查本地 tag 指向：`rtk git rev-parse vX.Y.Z^{}`。
3. 检查远端 tag：`rtk git ls-remote origin refs/tags/vX.Y.Z refs/tags/vX.Y.Z^{}`。
4. 向用户说明 tag 指向的 commit 和当前 release commit。
5. 不要删除、覆盖、force push tag，除非用户明确要求。

## 通用仓库适配

优先使用仓库自带 release 脚本。如果没有脚本，先识别所有版本 metadata 文件，再修改。常见文件：

| 生态 | Metadata |
| --- | --- |
| npm | `package.json`, lockfile |
| Rust | `Cargo.toml`, `Cargo.lock` |
| Tauri | `src-tauri/tauri.conf.json`, Rust metadata |
| Python | `pyproject.toml`, lockfile |

修改后运行仓库版本一致性检查，或做针对性的 diff 审计。

## 构建证据

汇报内容必须包含：

- 目标版本
- commit hash
- tag 名称
- commit 和 tag 的 push 结果
- 构建命令和通过/失败结果
- DMG 产物路径和文件名；如果只生成 `.app` 或 `.zip`，说明未完成 DMG 发布构建

如果 tag 推送后构建失败，必须说明 tag 已推送，并给出失败命令和错误。除非用户明确要求，不要改写或删除 tag。

如果执行过程中发现 skill 流程不符合实际仓库行为，先修正 skill，再继续发布流程。

## 常见错误

| 错误 | 修正 |
| --- | --- |
| 用户没给版本，agent 直接升级 | 先建议版本并等待确认 |
| 检查通过前打 tag | commit/tag 前先跑版本一致性检查 |
| 推了 tag 但跳过构建 | 构建是完成标准的一部分 |
| 用 `--bundle app` 当完整发布构建 | 改用 `scripts/release-pipeline.sh --bundle dmg --clean-required`，并确认 DMG 产物 |
| tag 已存在还继续发布 | 停止，查本地和远端 tag 指向，等待用户决策 |
| push 后构建失败 | 清楚报告部分发布状态；破坏性清理前先问用户 |
| 存在无关 dirty 文件 | 不纳入无关变更；若阻塞干净 release commit，先询问用户 |
