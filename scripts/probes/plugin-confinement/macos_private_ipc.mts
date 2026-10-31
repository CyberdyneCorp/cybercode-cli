/** Actual owned-listener observations for the experimental macOS profile. */
import assert from "node:assert/strict";
import { execFile, spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { closeSync, constants, fstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, readSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { release } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs, promisify } from "node:util";
import { policy } from "./macos_policy.mts";
import { datagramOwner } from "./macos_datagram_owner.mts";

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
  const target = join(packagePath, "guest"), helper = join(root, "helper"), datagram = join(root, "datagram-owner");
  for (const [file, output] of [["fixture_native.c", target], ["macos_exec.c", helper], ["macos_datagram_owner.c", datagram]]) {
    success("/usr/bin/xcrun", ["clang", "-Wall", "-Wextra", "-Werror", "-Wno-error=deprecated-declarations", "-pthread", join(source, file), "-o", output]);
  }
  const identifier = `com.cyberdyne.probe.unix.helper.${randomUUID()}`;
  success("/usr/bin/codesign", ["--force", "--sign", "-", "--identifier", identifier, "--timestamp=none", helper]);
  success("/usr/bin/codesign", ["--verify", "--strict", "--all-architectures", helper]);
  return { target, helper, identifier, packagePath, datagram };
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

function failureLog(base: string, pid: number) {
  if (!Number.isSafeInteger(pid) || pid <= 0 || pid > 0x7fffffff) return { pid: null, error: "invalid fixture PID" };
  const predicate = `process == "kernel" AND eventMessage CONTAINS ${JSON.stringify(`(${pid})`)} AND eventMessage CONTAINS ${JSON.stringify(base)}`;
  const result = spawnSync("/usr/bin/log", ["show", "--last", "1m", "--style", "json", "--predicate", predicate], {
    env: environment, encoding: "utf8", timeout: 5000, killSignal: "SIGKILL", maxBuffer: 65536,
  });
  return { pid, status: result.status, signal: result.signal,
    error: result.error?.message.slice(0, 2048) ?? "", stderr: result.stderr?.slice(0, 2048) ?? "",
    output: result.stdout?.slice(0, 16384) ?? "" };
}

async function row(root: string, prepared: ReturnType<typeof prepare>, name: string, flags: Flags, missingScope: boolean, dropRights: boolean) {
  const base = join(root, name);
  mkdirSync(base, { mode: 0o700 });
  const paths: Record<string, string> = { package: prepared.packagePath };
  for (const key of ["project", "private", "protected", "scratch"]) {
    paths[key] = join(base, key);
    mkdirSync(paths[key], { mode: 0o700 });
  }
  const host = join(paths.project, "host.sock");
  const listener = await owner(host);
  let payload = -1;
  let datagram: Awaited<ReturnType<typeof datagramOwner>> | undefined;
  try {
    const datagramHost = join(paths.project, "dgram-host.sock");
    datagram = await datagramOwner(prepared.datagram, datagramHost, join(paths.scratch, "dgram-client.sock"));
    payload = openSync(join(paths.scratch, "rights.bin"), constants.O_RDWR | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
    let profile = policy({ kind: "native", executable: prepared.target, libdir: "/usr/lib" }, paths, flags, false, prepared.identifier, true);
    if (missingScope) {
      const before = `(remote unix-socket (subpath ${JSON.stringify(paths.scratch)}))`;
      assert.equal(profile.split(before).length, 2);
      profile = profile.replace(before, '(remote unix-socket (subpath "/"))');
    }
    const file = join(root, `policy-${name}.sb`);
    writeFileSync(file, profile, { mode: 0o600 });
    const result = await execute(prepared.helper, [flags.unrestricted ? "-" : file, prepared.target, "unix-boundary", paths.scratch, host, datagramHost], {
      cwd: paths.scratch, env: { ...environment, ...dropRights && !flags.unrestricted ? { CYBER_PROOF_DROP_RIGHTS: "1" } : {} },
      encoding: "utf8", timeout: 15_000, maxBuffer: 65536,
    });
    const observed = JSON.parse(result.stdout);
    const receipt = listener.receipt();
    const datagram_receipts = await datagram.stop();
    assert(!receipt.invalid, "invalid owned-listener protocol");
    const identity = fstatSync(payload);
    assert(identity.isFile() && identity.size <= 2, "invalid owned rights payload");
    const contents = Buffer.alloc(3);
    const count = readSync(payload, contents, 0, contents.length, 0);
    assert(count <= 2, "oversized owned rights payload");
    const rights_payload = contents.subarray(0, count).toString();
    const kernel_diagnostic = observed.datagram_race.failure_stage ? failureLog(base, observed.pid) : undefined;
    return { name, flags, observed, rights_payload, rights_link_count: identity.nlink, kernel_diagnostic,
      owner_receipts: receipt.count, owner_received_fixture: receipt.count > 0,
      datagram_receipts, owner_datagram_received_fixture: datagram_receipts > 0 };
  } finally {
    try { if (datagram) await datagram.stop(); }
    finally {
      try { await listener.close(); }
      finally { if (payload >= 0) closeSync(payload); }
    }
  }
}

function check(entry: Awaited<ReturnType<typeof row>>) {
  assert.equal(entry.observed.started, true);
  assert.deepEqual(entry.observed.private_unix, [true, true], `private exchange: ${JSON.stringify(entry)}`);
  assert.deepEqual(entry.observed.private_aliases, [true, true], `private aliases: ${JSON.stringify(entry)}`);
  assert.equal(entry.observed.private_peer_checks, 8);
  assert.equal(entry.observed.private_peer_matches, true, `private peer identity: ${JSON.stringify(entry)}`);
  assert.equal(entry.observed.rights_path_replaced, true);
  assert.equal(entry.rights_link_count, 0, `original owner payload remained linked: ${JSON.stringify(entry)}`);
  assert.deepEqual(entry.observed.rights, {
    sent: true, original_closed: true, token: true, truncated: false, received: true,
    same_inode: true, readable: true, written: true, cloexec: true,
  }, `private SCM_RIGHTS exchange failed: ${JSON.stringify(entry)}`);
  assert.equal(entry.rights_payload, "AB", `received descriptor file effect: ${JSON.stringify(entry)}`);
  assert.deepEqual(entry.observed.private_datagram, [true, true, true], `private datagram exchange: ${JSON.stringify(entry)}`);
  assert.equal(entry.observed.datagram_symlink_created, true);
  const race = entry.observed.datagram_race;
  assert.equal(race.private_control, true, `private datagram race control failed: ${JSON.stringify(entry)}`);
  assert.equal(race.settled, true, `datagram mutator did not settle: ${JSON.stringify(entry)}`);
  assert.equal(race.failed, false, `datagram mutation failed: ${JSON.stringify(entry)}`);
  assert.equal(race.failure_stage, "");
  assert.equal(race.failure_errno, 0);
  assert.equal(race.failure_return, null);
  assert.equal(race.attempts, 16);
  assert(race.swaps >= race.attempts && race.swaps <= 4096);
  assert.equal(race.private_acks + race.host_acks + race.refused, race.attempts);
  assert.equal(entry.datagram_receipts, entry.flags.unrestricted ? 7 + race.host_acks : 0,
    `macOS datagram endpoint escaped scratch: ${JSON.stringify(entry)}`);
  assert.equal(race.host_control, !!entry.flags.unrestricted);
  assert.equal(race.refused, entry.flags.unrestricted ? 0 : race.attempts - race.private_acks);
  for (const prefix of ["dgram_", "dgram_alias_"]) for (const method of ["connect", "sendto", "sendmsg"]) {
    assert.deepEqual(entry.observed[prefix + method], {
      connected: !!entry.flags.unrestricted && method === "connect",
      sent: !!entry.flags.unrestricted, acknowledged: !!entry.flags.unrestricted,
    }, `host datagram route: ${prefix}${method}: ${JSON.stringify(entry)}`);
  }
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

async function startupControls(root: string, program: string) {
  const client = join(root, "startup-client.sock");
  await assert.rejects(datagramOwner(join(root, "missing-owner"), join(root, "missing.sock"), client), { code: "ENOENT" });
  const occupied = join(root, "occupied.sock");
  writeFileSync(occupied, "owned fixture", { mode: 0o600 });
  await assert.rejects(datagramOwner(program, occupied, client), /closed before ready: 2/);
  assert.equal(readFileSync(occupied, "utf8"), "owned fixture", "failed bind changed an existing owned file");
}

assert.equal(process.platform, "darwin", "macOS native UNIX boundary proof requires Darwin");
const { values } = parseArgs({ options: { report: { type: "string" }, "missing-scope": { type: "boolean", default: false }, "drop-rights": { type: "boolean", default: false } } });
assert(!(values["missing-scope"] && values["drop-rights"]), "select one precise control");
const root = realpathSync(mkdtempSync("/tmp/cyber-unix-"));
try {
  const prepared = prepare(root);
  await startupControls(root, prepared.datagram);
  const targetHash = createHash("sha256").update(readFileSync(prepared.target)).digest("hex");
  const rows = [];
  const cases: Array<[string, Flags]> = [["unconfined", { read: true, write: true, shell: true, unrestricted: true }]];
  for (const read of [false, true]) for (const write of [false, true]) for (const shell of [false, true]) {
    cases.push([`r${+read}w${+write}s${+shell}`, { read, write, shell }]);
  }
  for (const [name, flags] of cases) {
    const observed = await row(root, prepared, name, flags, values["missing-scope"], values["drop-rights"]);
    check(observed);
    rows.push(observed);
  }
  assert.equal(createHash("sha256").update(readFileSync(prepared.target)).digest("hex"), targetHash);
  const sources = ["macos_private_ipc.mts", "macos_policy.mts", "macos_rights.h", "macos_datagram_send.h", "macos_datagram.h", "macos_datagram_race.h", "macos_datagram_owner.mts", "macos_datagram_owner.c", "fixture_native.c", "process_policy.h", "macos_exec.c"];
  const hash = createHash("sha256");
  for (const file of sources) hash.update(file + "\0").update(readFileSync(join(source, file)));
  const report = { recorded_at: new Date().toISOString(), platform: process.platform, architecture: process.arch,
    kernel_release: release(), sources, source_sha256: hash.digest("hex"), target_sha256: targetHash,
    receiver_startup_controls: { missing_program_refused: true, failed_bind_refused: true }, rows,
    scope: "Experimental pre-runtime native UNIX stream/datagram/descriptor controls; complete IPC/proxy/ownership and App acceptance remain required." };
  if (values.report) writeFileSync(values.report, JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
  console.log(JSON.stringify(report, null, 2));
} finally {
  rmSync(root, { recursive: true, force: true });
}
