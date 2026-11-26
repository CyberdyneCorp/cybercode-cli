// Teardown for end-to-end tests that run a registered `cyber` service in a private
// CYBER_HOME. `cyber service stop` can fail (a service stopped at a process-cut fixture
// point, a damaged registration), and a test killed before its `after` hooks never runs
// them; either used to leave `cyber serve --register` running for days.
import { execFile, execFileSync } from "node:child_process";
import { readFileSync, realpathSync } from "node:fs";
import { join } from "node:path";

/** The registered service's process ID, when it is still this binary's `serve`. */
function registeredService(cyberHome: string, binary: string): number | undefined {
  let pid: unknown;
  try {
    pid = JSON.parse(readFileSync(join(cyberHome, "state", "server.json"), "utf8")).pid;
  } catch {
    return undefined;
  }
  if (typeof pid !== "number" || !Number.isInteger(pid) || pid <= 0 || pid === process.pid) return undefined;
  // A registration left by a killed service may name a reused process ID.
  let command: string;
  try {
    command = execFileSync("ps", ["-o", "command=", "-p", String(pid)], { encoding: "utf8" });
  } catch {
    return undefined;
  }
  let executable = binary;
  try {
    executable = realpathSync(binary);
  } catch {
    // Compare with the path as given.
  }
  const ours = command.includes(executable) || command.includes(binary);
  return ours && command.includes(" serve") ? pid : undefined;
}

/** SIGKILL the service registered in `cyberHome` if it still runs. Unix only. */
export function killRegisteredService(cyberHome: string, binary: string): void {
  if (process.platform === "win32") return;
  const pid = registeredService(cyberHome, binary);
  if (pid === undefined) return;
  try {
    process.kill(pid, "SIGKILL");
  } catch {
    // Already gone.
  }
}

/**
 * Kill the registered service when the returned function runs (call it in `t.after`
 * before removing the home) or, failing that, when this test process exits.
 */
export function guardRegisteredService(cyberHome: string, binary: string): () => void {
  const kill = () => killRegisteredService(cyberHome, binary);
  process.once("exit", kill);
  return () => {
    process.off("exit", kill);
    kill();
  };
}

/**
 * Stop the service registered in `cyberHome` gracefully, then kill it if it still runs.
 * Never throws, so the rest of a teardown always runs.
 */
export async function stopRegisteredService(cyberHome: string, binary: string): Promise<void> {
  await new Promise<void>((resolve) => {
    execFile(binary, ["service", "stop"], { env: { ...process.env, CYBER_HOME: cyberHome }, timeout: 30_000 }, () => resolve());
  });
  killRegisteredService(cyberHome, binary);
}
