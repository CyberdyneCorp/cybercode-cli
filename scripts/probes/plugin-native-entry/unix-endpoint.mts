/** Endpoint identity prerequisite, not a production broker or confinement test. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { release } from "node:os";
import { parseArgs } from "node:util";

type Observation = {
  type: "stream" | "datagram" | "seqpacket";
  pinned: boolean; stage: string; errno: number;
  replaced: boolean; symlink_refused: boolean; escape_refused: boolean;
  absolute_refused: boolean; kind_refused: boolean; identity_retained: boolean;
  private_received: boolean; ambient_received: boolean; roundtrip: boolean; passed: boolean;
};
function verify(rows: Observation[]): void {
  const expected = new Set(["stream", "datagram", "seqpacket"].flatMap(type =>
    [false, true].map(pinned => `${type}:${pinned}`)));
  assert.equal(rows.length, expected.size);
  for (const row of rows) {
    assert.equal(typeof row.pinned, "boolean");
    assert(expected.delete(`${row.type}:${row.pinned}`), "missing, duplicate or unexpected case");
    assert.equal(row.stage, "complete"); assert.equal(row.errno, 0);
    for (const key of ["replaced", "symlink_refused", "escape_refused", "absolute_refused",
      "kind_refused", "identity_retained", "roundtrip", "passed"] as const) {
      assert.equal(row[key], true, `${row.type} ${row.pinned}: ${key}`);
    }
    assert.equal(row.private_received, row.pinned, "pinned identity must select the private endpoint");
    assert.equal(row.ambient_received, !row.pinned, "re-resolving must expose the fixture counterexample");
  }
  assert.equal(expected.size, 0);
}
function selfTest(): void {
  const rows: Observation[] = (["stream", "datagram", "seqpacket"] as const).flatMap(type =>
    [false, true].map(pinned => ({ type, pinned, stage: "complete", errno: 0,
      replaced: true, symlink_refused: true, escape_refused: true, absolute_refused: true,
      kind_refused: true, identity_retained: true, private_received: pinned,
      ambient_received: !pinned, roundtrip: true, passed: true })));
  verify(rows);
  assert.throws(() => verify(rows.slice(1)));
  assert.throws(() => verify(rows.map(row => ({ ...row, ambient_received: false }))));
  assert.throws(() => verify(rows.map(row => ({ ...row, identity_retained: false }))));
  assert.throws(() => verify(rows.map(row => ({ ...row, symlink_refused: false }))));
  assert.throws(() => verify([...rows.slice(1), rows[1]]));
}
const { values } = parseArgs({ options: { probe: { type: "string" }, report: { type: "string" },
  "self-test": { type: "boolean" } } });
selfTest();
if (!values["self-test"]) {
  assert.equal(process.platform, "linux"); assert(values.probe && values.report);
  const rows: { mode: string; status: number | null; observed: Observation[] }[] = [];
  let passed = false;
  try {
    for (const mode of ["host", "private-namespaces"]) {
      const program = mode === "host" ? values.probe : "/usr/bin/bwrap";
      const args = mode === "host" ? [] : ["--die-with-parent", "--unshare-user", "--unshare-pid",
        "--unshare-net", "--unshare-ipc", "--unshare-uts", "--disable-userns", "--cap-drop", "ALL",
        "--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp",
        "--", values.probe];
      const result = spawnSync(program, args, { encoding: "utf8", timeout: 20_000,
        maxBuffer: 64 * 1024, env: { PATH: "/usr/bin:/bin" } });
      assert(!result.error, `native observation failed: ${result.error}`);
      const observed = result.stdout.trim().split("\n").filter(Boolean)
        .map(line => JSON.parse(line) as Observation);
      rows.push({ mode, status: result.status, observed });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      verify(observed);
    }
    passed = true;
  } finally {
    const sources = ["scripts/probes/plugin-native-entry/linux_unix_endpoint_pin.c",
      "scripts/probes/plugin-native-entry/unix-endpoint.mts", ".github/workflows/plugin-linux-ipc.yml"];
    const hash = createHash("sha256");
    for (const path of sources) hash.update(path + "\0").update(readFileSync(path));
    writeFileSync(values.report, JSON.stringify({ platform: process.platform, architecture: process.arch,
      kernel_release: release(), checkout: process.env.GITHUB_SHA ?? null, run_id: process.env.GITHUB_RUN_ID ?? null,
      passed, sources, source_sha256: hash.digest("hex"),
      probe_sha256: createHash("sha256").update(readFileSync(values.probe)).digest("hex"), rows,
      scope: "Owned-fixture O_PATH/openat2 endpoint identity and replacement counterexample only. No syscall broker integration, full IPC policy or production acceptance." }, null, 2) + "\n");
    console.log(JSON.stringify({ passed, native_policy_acceptance: false,
      cases: rows.map(row => ({ mode: row.mode, count: row.observed.length })) }));
  }
}
