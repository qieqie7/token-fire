import assert from "node:assert/strict";
import test from "node:test";
import { formatIdentityEnv } from "./build-identity-env.mjs";

test("formatIdentityEnv emits shell assignments for build metadata", () => {
  const output = formatIdentityEnv({
    gitCommit: "7e17eb0abcdef",
    gitCommitShort: "7e17eb0",
    dirty: false,
    buildTime: "unix:123",
  });

  assert.match(output, /^TOKEN_FIRE_GIT_COMMIT=7e17eb0abcdef$/m);
  assert.match(output, /^TOKEN_FIRE_GIT_COMMIT_SHORT=7e17eb0$/m);
  assert.match(output, /^TOKEN_FIRE_GIT_DIRTY=false$/m);
  assert.match(output, /^TOKEN_FIRE_BUILD_TIME=unix:123$/m);
});
