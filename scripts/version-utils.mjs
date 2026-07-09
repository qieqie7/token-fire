import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const SEMVER_RE = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

export function assertSemver(version) {
  if (!SEMVER_RE.test(version)) {
    throw new Error(
      `TokenFire release version must be SemVer MAJOR.MINOR.PATCH, got ${version}`,
    );
  }
  return version;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

export function readVersions(root = process.cwd()) {
  const packageJson = readJson(join(root, "package.json"));
  const tauriConfig = readJson(join(root, "src-tauri", "tauri.conf.json"));
  const cargoToml = readFileSync(join(root, "src-tauri", "Cargo.toml"), "utf8");
  const cargoLock = readFileSync(join(root, "src-tauri", "Cargo.lock"), "utf8");
  const packageStart = cargoToml.indexOf("[package]");
  const nextSectionStart = cargoToml.indexOf("\n[", packageStart + "[package]".length);
  const packageSection =
    packageStart >= 0
      ? cargoToml.slice(packageStart, nextSectionStart === -1 ? undefined : nextSectionStart)
      : "";
  const cargoVersion = packageSection?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  const lockPackageStart = cargoLock.search(/\[\[package\]\]\nname = "token-fire"\n/);
  const lockNextPackageStart = cargoLock.indexOf("\n[[package]]", lockPackageStart + 1);
  const lockPackageSection =
    lockPackageStart >= 0
      ? cargoLock.slice(lockPackageStart, lockNextPackageStart === -1 ? undefined : lockNextPackageStart)
      : "";
  const cargoLockVersion = lockPackageSection?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (typeof packageJson.version !== "string") {
    throw new Error("package.json version is missing");
  }
  if (typeof tauriConfig.version !== "string") {
    throw new Error("src-tauri/tauri.conf.json version is missing");
  }
  if (typeof cargoVersion !== "string") {
    throw new Error("src-tauri/Cargo.toml [package] version is missing");
  }
  if (typeof cargoLockVersion !== "string") {
    throw new Error("src-tauri/Cargo.lock token-fire version is missing");
  }
  return {
    packageJson: packageJson.version,
    cargoToml: cargoVersion,
    tauriConfig: tauriConfig.version,
    cargoLock: cargoLockVersion,
  };
}

export function assertVersionsConsistent(versions) {
  for (const [file, version] of Object.entries(versions)) {
    assertSemver(version);
    if (version !== versions.packageJson) {
      throw new Error(
        `TokenFire version drift: package.json=${versions.packageJson}, ${file}=${version}`,
      );
    }
  }
  return versions.packageJson;
}

export function nextVersion(current, bump) {
  assertSemver(current);
  const match = current.match(SEMVER_RE);
  const major = Number(match[1]);
  const minor = Number(match[2]);
  const patch = Number(match[3]);
  if (bump === "patch") return `${major}.${minor}.${patch + 1}`;
  if (bump === "minor") return `${major}.${minor + 1}.0`;
  if (bump === "major") return `${major + 1}.0.0`;
  throw new Error(`Unsupported version bump ${bump}`);
}

function replaceJsonVersion(path, version, missingMessage) {
  const body = readFileSync(path, "utf8");
  const nextBody = body.replace(/(^\s*"version"\s*:\s*")([^"]+)(")/m, `$1${version}$3`);
  if (nextBody === body && readJson(path).version !== version) {
    throw new Error(missingMessage);
  }
  writeFileSync(path, nextBody);
}

export function setVersions(root, version) {
  assertSemver(version);
  const packagePath = join(root, "package.json");
  const tauriConfigPath = join(root, "src-tauri", "tauri.conf.json");
  const cargoPath = join(root, "src-tauri", "Cargo.toml");
  const cargoLockPath = join(root, "src-tauri", "Cargo.lock");

  replaceJsonVersion(packagePath, version, "Failed to update package.json version");
  replaceJsonVersion(
    tauriConfigPath,
    version,
    "Failed to update src-tauri/tauri.conf.json version",
  );

  const cargoToml = readFileSync(cargoPath, "utf8");
  const nextCargoToml = cargoToml.replace(
    /(^\[package\]\n[\s\S]*?^version\s*=\s*")([^"]+)(")/m,
    `$1${version}$3`,
  );
  if (nextCargoToml === cargoToml && readVersions(root).cargoToml !== version) {
    throw new Error("Failed to update src-tauri/Cargo.toml [package] version");
  }
  writeFileSync(cargoPath, nextCargoToml);

  const cargoLock = readFileSync(cargoLockPath, "utf8");
  const nextCargoLock = cargoLock.replace(
    /(\[\[package\]\]\nname = "token-fire"\nversion = ")([^"]+)(")/m,
    `$1${version}$3`,
  );
  if (nextCargoLock === cargoLock && readVersions(root).cargoLock !== version) {
    throw new Error("Failed to update src-tauri/Cargo.lock token-fire version");
  }
  writeFileSync(cargoLockPath, nextCargoLock);
}
