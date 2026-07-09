#!/usr/bin/env node
import { execFileSync } from "node:child_process";

function git(args) {
  return execFileSync("git", args, { encoding: "utf8" }).trim();
}

export function formatIdentityEnv({ gitCommit, gitCommitShort, dirty, buildTime }) {
  return [
    `TOKEN_FIRE_GIT_COMMIT=${gitCommit}`,
    `TOKEN_FIRE_GIT_COMMIT_SHORT=${gitCommitShort}`,
    `TOKEN_FIRE_GIT_DIRTY=${dirty ? "true" : "false"}`,
    `TOKEN_FIRE_BUILD_TIME=${buildTime}`,
  ].join("\n");
}

export function currentIdentityEnv() {
  const gitCommit = git(["rev-parse", "HEAD"]);
  const gitCommitShort = git(["rev-parse", "--short=7", "HEAD"]);
  const status = git(["status", "--porcelain", "--untracked-files=normal"]);
  return {
    gitCommit,
    gitCommitShort,
    dirty: status.trim().length > 0,
    buildTime: `unix:${Math.floor(Date.now() / 1000)}`,
  };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  console.log(formatIdentityEnv(currentIdentityEnv()));
}
