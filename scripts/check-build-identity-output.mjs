#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const [appPath, hookPath, expectedVersion, expectedCommit, expectedShort, expectedDirty] =
  process.argv.slice(2);

if (!appPath || !hookPath || !expectedVersion || !expectedCommit || !expectedShort || !expectedDirty) {
  console.error(
    "Usage: node scripts/check-build-identity-output.mjs <app.json> <hook.json> <version> <commit> <short> <dirty>",
  );
  process.exit(1);
}

const app = JSON.parse(readFileSync(appPath, "utf8"));
const hook = JSON.parse(readFileSync(hookPath, "utf8"));

for (const [name, identity] of [
  ["app", app],
  ["hook", hook],
]) {
  assert.equal(identity.version, expectedVersion, `${name} version mismatch`);
  assert.equal(identity.git_commit, expectedCommit, `${name} git_commit mismatch`);
  assert.equal(identity.git_commit_short, expectedShort, `${name} git_commit_short mismatch`);
  assert.equal(String(identity.dirty), expectedDirty, `${name} dirty mismatch`);
  assert.equal(
    identity.git_commit.startsWith(identity.git_commit_short),
    true,
    `${name} short commit is not a prefix`,
  );
  assert.equal(typeof identity.build_time, "string", `${name} build_time missing`);
  assert.notEqual(identity.build_time.length, 0, `${name} build_time empty`);
}

for (const field of ["version", "git_commit", "git_commit_short", "dirty", "build_time"]) {
  assert.deepEqual(app[field], hook[field], `app/hook ${field} mismatch`);
}

console.log(`TokenFire build identities match ${expectedVersion} ${expectedShort}`);
