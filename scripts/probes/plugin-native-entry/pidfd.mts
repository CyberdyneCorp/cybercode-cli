/** Descriptor-pinning prerequisites only; no production confinement acceptance. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { release } from "node:os";
import { parseArgs } from "node:util";

type Observation = {
  outcome: "passed" | "unavailable" | "failed"; stage: string; errno: number;
  leader_domain: number; thread_domain: number; replacement_domain: number; pinned_domain: number;
  copied_address_changed: boolean; guest_observed_bind: boolean; pinned_tcp_roundtrip: boolean;
  thread_after_exit_errno: number; nondumpable_fd_errno: number; nondumpable_memory_errno: number;
};
function verify(row: Observation): void {
  assert(["passed", "unavailable"].includes(row.outcome), `native prerequisite failed: ${JSON.stringify(row)}`);
  if (row.outcome === "unavailable") {
    assert(["unshare_files", "pidfd_process", "pidfd_thread", "duplicate_process", "duplicate_thread",
      "copy_guest_memory", "duplicate_replacement"].includes(row.stage));
    assert([1, 22, 38].includes(row.errno), "unavailability must report a specific permission/kernel refusal");
    return;
  }
  assert.equal(row.stage, "complete"); assert.equal(row.errno, 0);
  assert.equal(row.leader_domain, 1, "TGID lookup must expose the wrong UNIX socket counterexample");
  assert.equal(row.thread_domain, 2, "thread pidfd must select its independent INET table");
  assert.equal(row.replacement_domain, 1); assert.equal(row.pinned_domain, 2);
  for (const key of ["copied_address_changed", "guest_observed_bind", "pinned_tcp_roundtrip"] as const) {
    assert.equal(row[key], true, key);
  }
  assert.equal(row.thread_after_exit_errno, 3, "thread handle must fail after that thread exits");
  assert.equal(row.nondumpable_fd_errno, 1); assert.equal(row.nondumpable_memory_errno, 1);
}
function selfTest(): void {
  const good: Observation = { outcome: "passed", stage: "complete", errno: 0,
    leader_domain: 1, thread_domain: 2, replacement_domain: 1, pinned_domain: 2,
    copied_address_changed: true, guest_observed_bind: true, pinned_tcp_roundtrip: true,
    thread_after_exit_errno: 3, nondumpable_fd_errno: 1, nondumpable_memory_errno: 1 };
  verify(good);
  verify({ ...good, outcome: "unavailable", stage: "duplicate_thread", errno: 1 });
  assert.throws(() => verify({ ...good, pinned_domain: 1 }));
  assert.throws(() => verify({ ...good, thread_after_exit_errno: 0 }));
  assert.throws(() => verify({ ...good, outcome: "unavailable", stage: "pinned_tcp_roundtrip", errno: 1 }));
  assert.throws(() => verify({ ...good, outcome: "unavailable", stage: "duplicate_thread", errno: 12 }));
}
const { values } = parseArgs({ options: { probe: { type: "string" }, report: { type: "string" },
  "self-test": { type: "boolean" } } });
selfTest();
if (!values["self-test"]) {
  assert.equal(process.platform, "linux"); assert(values.probe && values.report);
  const rows: { mode: string; status: number | null; observed: Observation }[] = [];
  let outcome = "failed";
  try {
    for (const mode of ["host", "private-namespaces"]) {
      const program = mode === "host" ? values.probe : "/usr/bin/bwrap";
      const args = mode === "host" ? [] : ["--die-with-parent", "--unshare-user", "--unshare-pid",
        "--unshare-net", "--unshare-ipc", "--unshare-uts", "--disable-userns", "--cap-drop", "ALL",
        "--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev", "--", values.probe];
      const result = spawnSync(program, args, { encoding: "utf8", timeout: 20_000,
        maxBuffer: 64 * 1024, env: { PATH: "/usr/bin:/bin" } });
      assert(!result.error, `native process observation failed: ${result.error}`);
      const observed: Observation = JSON.parse(result.stdout);
      rows.push({ mode, status: result.status, observed });
      assert.equal(result.status, 0, result.stderr);
      verify(observed);
    }
    outcome = rows.every(row => row.observed.outcome === "passed") ? "prerequisites-passed" : "prerequisites-unavailable";
  } finally {
    const sources = ["scripts/probes/plugin-native-entry/linux_pidfd_probe.c",
      "scripts/probes/plugin-native-entry/pidfd.mts", ".github/workflows/plugin-linux-ipc.yml"];
    const hash = createHash("sha256");
    for (const path of sources) hash.update(path + "\0").update(readFileSync(path));
    writeFileSync(values.report, JSON.stringify({ platform: process.platform, architecture: process.arch,
      kernel_release: release(), checkout: process.env.GITHUB_SHA ?? null, run_id: process.env.GITHUB_RUN_ID ?? null,
      outcome, sources, source_sha256: hash.digest("hex"),
      probe_sha256: createHash("sha256").update(readFileSync(values.probe)).digest("hex"), rows,
      scope: "Owned-fixture syscall prerequisites only. No syscall broker, sandbox policy, runtime support revision or production acceptance." }, null, 2) + "\n");
    console.log(JSON.stringify({ outcome, native_policy_acceptance: false,
      observations: rows.map(row => ({ mode: row.mode, outcome: row.observed.outcome, stage: row.observed.stage, errno: row.observed.errno })) }));
  }
}
