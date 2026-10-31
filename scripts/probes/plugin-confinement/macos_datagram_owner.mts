/** Bounded native receiver ownership for fixture-only UNIX datagrams. */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { dirname } from "node:path";

export async function datagramOwner(program: string, path: string, client: string) {
  const child = spawn(program, [path, client], {
    cwd: dirname(path), env: { PATH: "/usr/bin:/bin:/usr/sbin:/sbin" }, stdio: ["pipe", "pipe", "pipe"],
  });
  let stdout = "", stderr = "", failed: Error | undefined, ready = false;
  let admit!: () => void, refuse!: (error: Error) => void;
  const started = new Promise<void>((resolve, reject) => { admit = resolve; refuse = reject; });
  const deadline = setTimeout(() => {
    failed = new Error("owned datagram receiver timed out");
    child.kill("SIGKILL");
  }, 32_000);
  const completed = new Promise<{ code: number | null; signal: string | null }>(resolve => {
    child.on("error", error => { failed = error; });
    child.once("close", (code, signal) => {
      clearTimeout(deadline);
      if (!ready) refuse(failed ?? new Error(`datagram receiver closed before ready: ${code}: ${stderr}`));
      resolve({ code, signal });
    });
  });
  child.stdin.on("error", error => { failed = error; });
  child.stdout.on("data", (bytes: Buffer) => {
    if (stdout.length + bytes.length > 4096) {
      failed = new Error("owned datagram output exceeds bound"); child.kill("SIGKILL"); return;
    }
    stdout += bytes.toString();
    if (!ready && stdout.startsWith("READY\n")) { ready = true; admit(); }
  });
  child.stderr.on("data", (bytes: Buffer) => {
    if (stderr.length + bytes.length > 4096) {
      failed = new Error("owned datagram error output exceeds bound"); child.kill("SIGKILL"); return;
    }
    stderr += bytes.toString();
  });
  try { await started; }
  catch (error) { child.kill("SIGKILL"); await completed; throw error; }
  let stopping: Promise<number> | undefined;
  return {
    stop: () => stopping ??= (async () => {
      if (child.exitCode === null && child.signalCode === null) child.stdin.end("S");
      const outcome = await completed;
      if (failed) throw failed;
      assert.equal(outcome.code, 0, JSON.stringify({ ...outcome, stderr }));
      const lines = stdout.trim().split("\n");
      assert.equal(lines.length, 2, "unexpected datagram owner transcript");
      assert.equal(lines[0], "READY");
      const report = JSON.parse(lines[1]);
      assert.equal(report.stopped, true);
      assert(Number.isSafeInteger(report.receipts) && report.receipts >= 0 && report.receipts <= 32);
      return report.receipts as number;
    })(),
  };
}
