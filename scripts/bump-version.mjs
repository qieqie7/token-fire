#!/usr/bin/env node
import {
  assertSemver,
  assertVersionsConsistent,
  nextVersion,
  readVersions,
  setVersions,
} from "./version-utils.mjs";

const args = process.argv.slice(2);
const target = args[0] === "--" ? args[1] : args[0];

if (!target) {
  console.error("Usage: pnpm release:bump -- <patch|minor|major|MAJOR.MINOR.PATCH>");
  process.exit(1);
}

try {
  const current = assertVersionsConsistent(readVersions(process.cwd()));
  const next =
    target === "patch" || target === "minor" || target === "major" ? nextVersion(current, target) : assertSemver(target);
  setVersions(process.cwd(), next);
  console.log(`TokenFire version updated: ${current} -> ${next}`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
