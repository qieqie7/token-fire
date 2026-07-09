#!/usr/bin/env node
import { assertVersionsConsistent, readVersions } from "./version-utils.mjs";

try {
  const versions = readVersions(process.cwd());
  const version = assertVersionsConsistent(versions);
  console.log(`TokenFire versions consistent: ${version}`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
