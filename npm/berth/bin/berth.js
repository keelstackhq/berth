#!/usr/bin/env node
"use strict";

// npm launcher for berth: runs the binary from the platform package npm installed through
// optionalDependencies, with inherited stdio and the binary's exit code.

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

/** `process.platform-process.arch` -> platform package (one per release target). */
const PLATFORMS = {
  "linux-x64": "@keelstackhq/berth-linux-x64",
  "linux-arm64": "@keelstackhq/berth-linux-arm64",
  "darwin-x64": "@keelstackhq/berth-darwin-x64",
  "darwin-arm64": "@keelstackhq/berth-darwin-arm64",
  "win32-x64": "@keelstackhq/berth-win32-x64",
};

function packageFor(platform, arch) {
  return PLATFORMS[`${platform}-${arch}`];
}

/** Path of `bin/berth[.exe]` inside the platform package; throws when it is not installed. */
function binaryPath(platform, arch, resolve = require.resolve) {
  const pkg = packageFor(platform, arch);
  const dir = path.dirname(resolve(`${pkg}/package.json`));
  return path.join(dir, "bin", platform === "win32" ? "berth.exe" : "berth");
}

/** Run berth with `args`; returns the exit code. */
function run(args, resolve = require.resolve) {
  const { platform, arch } = process;
  const key = `${platform}-${arch}`;
  const pkg = packageFor(platform, arch);
  if (!pkg) {
    process.stderr.write(
      `berth: no binary for ${key}; supported platforms: ${Object.keys(PLATFORMS).join(", ")}\n`,
    );
    return 1;
  }
  let bin;
  try {
    bin = binaryPath(platform, arch, resolve);
  } catch {
    process.stderr.write(
      `berth: platform package ${pkg} is not installed; reinstall @keelstackhq/berth ` +
        `without --no-optional (or --omit=optional)\n`,
    );
    return 1;
  }
  let result = spawnSync(bin, args, { stdio: "inherit" });
  if (result.error && result.error.code === "EACCES") {
    // Some installers drop the executable bit; restore it once and retry.
    try {
      fs.chmodSync(bin, 0o755);
      result = spawnSync(bin, args, { stdio: "inherit" });
    } catch {
      // Keep the first error.
    }
  }
  if (result.error) {
    process.stderr.write(`berth: cannot run ${bin}: ${result.error.message}\n`);
    return 1;
  }
  if (result.signal) {
    process.kill(process.pid, result.signal);
    return 1;
  }
  return result.status ?? 1;
}

module.exports = { PLATFORMS, packageFor, binaryPath, run };

if (require.main === module) {
  process.exitCode = run(process.argv.slice(2));
}
