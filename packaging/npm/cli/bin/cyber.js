#!/usr/bin/env node
// Launcher of `@cyber-code/cli`: runs the cyber binary from the platform package npm
// installed through optionalDependencies (`@cyber-code/cli-<platform>-<arch>`), forwarding
// arguments, stdio, the exit code and SIGINT/SIGTERM/SIGHUP.
"use strict";

const { spawn } = require("node:child_process");
const path = require("node:path");

const SIGNALS = ["SIGINT", "SIGTERM", "SIGHUP"];

function binaryPath() {
  const key = `${process.platform}-${process.arch}`;
  const name = `@cyber-code/cli-${key}`;
  let manifest;
  try {
    manifest = require.resolve(`${name}/package.json`);
  } catch {
    console.error(
      `cyber: no binary for ${key}; ${name} is not installed. ` +
        "Reinstall without --no-optional, or use the install script.",
    );
    process.exit(1);
  }
  const exe = process.platform === "win32" ? "cyber.exe" : "cyber";
  return path.join(path.dirname(manifest), "bin", exe);
}

function run() {
  const child = spawn(binaryPath(), process.argv.slice(2), { stdio: "inherit" });
  const forward = (signal) => {
    if (child.exitCode === null && child.signalCode === null) child.kill(signal);
  };
  for (const signal of SIGNALS) process.on(signal, forward);
  child.on("error", (error) => {
    console.error(`cyber: ${error.message}`);
    process.exit(1);
  });
  child.on("exit", (code, signal) => {
    for (const s of SIGNALS) process.off(s, forward);
    if (signal) {
      // Die of the same signal, so the caller sees what the binary saw.
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 1);
  });
}

run();
