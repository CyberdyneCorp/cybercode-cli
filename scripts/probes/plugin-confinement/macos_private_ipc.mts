/** Actual owned-listener observations for the experimental macOS profile. */
import assert from "node:assert/strict";
import { execFile, spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { release } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs, promisify } from "node:util";
import { policy } from "./macos_policy.mts";

const source = dirname(fileURLToPath(import.meta.url));
const environment = { PATH: "/usr/bin:/bin:/usr/sbin:/sbin" };
const execute = promisify(execFile);
type Flags = { read: boolean; write: boolean; shell: boolean; unrestricted?: boolean };

function success(program: string, args: string[]) {
  const result = spawnSync(program, args, { env: environment, encoding: "utf8", timeout: 30_000, maxBuffer: 65536 });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, JSON.stringify({ program, ...result }));
}

function prepare(root: string) {
  const packagePath = join(root, "package");
  mkdirSync(packagePath, { mode: 0o700 });
  const target = join(packagePath, "guest"), helper = join(root, "helper");
  for (const [file, output] of [["fixture_native.c", target], ["macos_exec.c", helper]]) {
    success("/usr/bin/xcrun", ["clang", "-Wall", "-Wextra", "-Werror", "-Wno-error=deprecated-declarations", "-pthread", join(source, file), "-o", output]);
  }
  const identifier = `com.cyberdyne.probe.unix.helper.${randomUUID()}`;
  success("/usr/bin/codesign", ["--force", "--sign", "-", "--identifier", identifier, "--timestamp=none", helper]);
  success("/usr/bin/codesign", ["--verify", "--strict", "--all-architectures", helper]);
  return { target, helper, identifier, packagePath };
}

async function owner(path: string) {
  const peers = new Set<import("node:net").Socket>();
  let receipts = 0, invalid = false;
  const server = createServer(peer => {
    peers.add(peer);
    peer.setTimeout(2000, () => peer.destroy());
    peer.on("close", () => peers.delete(peer));
    peer.on("error", () => {});
    let bytes = Buffer.alloc(0), complete = false;
    peer.on("data", data => {
      if (complete || bytes.length + data.length > 12) { invalid = true; peer.destroy(); return; }
      bytes = Buffer.concat([bytes, data]);
      if (bytes.length !== 12) return;
      complete = true;
      if (bytes.toString() !== "fixture-only") { invalid = true; peer.destroy(); return; }
      receipts++;
      peer.end("A");
    });
  });
  server.maxConnections = 8;
  server.on("error", () => { invalid = true; });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(path, resolve);
  });
  return {
    receipt: () => ({ count: receipts, invalid }),
    close: async () => {
      for (const peer of peers) peer.destroy();
      await new Promise<void>(resolve => server.close(() => resolve()));
    },
  };
}

async function row(root: string, prepared: ReturnType<typeof prepare>, name: string, flags: Flags, missingScope: boolean) {
  const base = join(root, name);
  mkdirSync(base, { mode: 0o700 });
  const paths: Record<string, string> = { package: prepared.packagePath };
  for (const key of ["project", "private", "protected", "scratch"]) {
    paths[key] = join(base, key);
    mkdirSync(paths[key], { mode: 0o700 });
  }
  const host = join(paths.project, "host.sock");
  const listener = await owner(host);
  try {
    let profile = policy({ kind: "native", executable: prepared.target, libdir: "/usr/lib" }, paths, flags, false, prepared.identifier, true);
    if (missingScope) {
      const before = `(remote unix-socket (subpath ${JSON.stringify(paths.scratch)}))`;
      assert.equal(profile.split(before).length, 2);
      profile = profile.replace(before, '(remote unix-socket (subpath "/"))');
    }
    const file = join(root, `policy-${name}.sb`);
    writeFileSync(file, profile, { mode: 0o600 });
    const result = await execute(prepared.helper, [flags.unrestricted ? "-" : file, prepared.target, "unix-boundary", paths.scratch, host], {
      cwd: paths.scratch, env: environment, encoding: "utf8", timeout: 15_000, maxBuffer: 65536,
    });
    const observed = JSON.parse(result.stdout);
    const receipt = listener.receipt();
    assert(!receipt.invalid, "invalid owned-listener protocol");
    return { name, flags, observed, owner_receipts: receipt.count, owner_received_fixture: receipt.count > 0 };
  } finally {
    await listener.close();
  }
}

function check(entry: Awaited<ReturnType<typeof row>>) {
  assert.equal(entry.observed.started, true);
  assert.deepEqual(entry.observed.private_unix, [true, true], `private exchange: ${JSON.stringify(entry)}`);
  assert.deepEqual(entry.observed.private_aliases, [true, true], `private aliases: ${JSON.stringify(entry)}`);
  assert.equal(entry.observed.private_peer_checks, 8);
  assert.equal(entry.observed.private_peer_matches, true, `private peer identity: ${JSON.stringify(entry)}`);
  if (entry.flags.unrestricted) {
    assert.equal(entry.observed.hardlink_created, true);
    assert.equal(entry.observed.symlink_created, true);
    assert.equal(entry.owner_receipts, 3, `owned-listener control: ${JSON.stringify(entry)}`);
  } else {
    assert.equal(entry.owner_received_fixture, false, `macOS UNIX socket boundary escaped scratch: ${JSON.stringify(entry)}`);
  }
  for (const route of ["direct", "hardlink", "symlink"]) {
    assert.deepEqual(entry.observed[route], {
      connected: !!entry.flags.unrestricted, sent: !!entry.flags.unrestricted, acknowledged: !!entry.flags.unrestricted,
    }, `host UNIX route: ${route}: ${JSON.stringify(entry)}`);
  }
}

assert.equal(process.platform, "darwin", "macOS native UNIX boundary proof requires Darwin");
const { values } = parseArgs({ options: { report: { type: "string" }, "missing-scope": { type: "boolean", default: false } } });
const root = realpathSync(mkdtempSync("/tmp/cyber-unix-"));
try {
  const prepared = prepare(root);
  const targetHash = createHash("sha256").update(readFileSync(prepared.target)).digest("hex");
  const rows = [];
  const cases: Array<[string, Flags]> = [["unconfined", { read: true, write: true, shell: true, unrestricted: true }]];
  for (const read of [false, true]) for (const write of [false, true]) for (const shell of [false, true]) {
    cases.push([`r${+read}w${+write}s${+shell}`, { read, write, shell }]);
  }
  for (const [name, flags] of cases) {
    const observed = await row(root, prepared, name, flags, values["missing-scope"]);
    check(observed);
    rows.push(observed);
  }
  assert.equal(createHash("sha256").update(readFileSync(prepared.target)).digest("hex"), targetHash);
  const sources = ["macos_private_ipc.mts", "macos_policy.mts", "fixture_native.c", "process_policy.h", "macos_exec.c"];
  const hash = createHash("sha256");
  for (const file of sources) hash.update(file + "\0").update(readFileSync(join(source, file)));
  const report = { recorded_at: new Date().toISOString(), platform: process.platform, architecture: process.arch,
    kernel_release: release(), sources, source_sha256: hash.digest("hex"), target_sha256: targetHash, rows,
    scope: "Experimental pre-runtime native UNIX stream controls only; complete IPC/proxy/ownership and App acceptance remain required." };
  if (values.report) writeFileSync(values.report, JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
  console.log(JSON.stringify(report, null, 2));
} finally {
  rmSync(root, { recursive: true, force: true });
}
