import { connect, type CyberClient } from "./client.js";
import type { ConnectOptions } from "./connection.js";
import { once, startEmbedded } from "./embedded.js";
import { readRegistration, type Registration } from "./local.js";

export interface StartOptions extends Omit<ConnectOptions, "baseUrl"> {
  /**
   * `attach` uses the registered server; `spawn` runs `cyber service start` first (reusing a
   * healthy service); `embedded` runs a private `cyber serve --stdio` tied to the handle.
   */
  mode: "attach" | "spawn" | "embedded";
  /** The `cyber` executable for `spawn` and `embedded` (default: `cyber` on `PATH`). */
  binary?: string;
  /** `embedded` only: database file for the private server; in-memory when omitted. */
  db?: string;
  /** Aborting closes the handle. */
  signal?: AbortSignal;
}

export interface CyberHandle {
  client: CyberClient;
  /** The registered server for `attach` and `spawn`; absent for `embedded`. */
  registration?: Registration;
  /**
   * `embedded`: stops the private server. `spawn`: stops the service when this handle started it.
   * `attach` or a reused service: a no-op.
   */
  close(): Promise<void>;
}

export async function start(options: StartOptions): Promise<CyberHandle> {
  if (options.mode === "embedded") return startEmbedded(options);
  const binary = options.binary ?? "cyber";
  const reused = options.mode === "spawn" ? await healthyRegistration(options) : undefined;
  if (options.mode === "spawn" && !reused) await run(binary, ["service", "start"]);
  const handle = await attach(options);
  if (options.mode === "attach" || reused) return handle;
  const close = once(() => run(binary, ["service", "stop"]));
  options.signal?.addEventListener("abort", () => void close().catch(() => {}), { once: true });
  return { ...handle, close };
}

async function attach(options: StartOptions): Promise<CyberHandle> {
  const registration = await readRegistration();
  if (!registration) throw new Error("no registered Cyber server; run `cyber service start` or use mode \"spawn\"");
  const client = connect({ ...options, baseUrl: registration.url });
  await client.health.get({ signal: options.signal });
  return { client, registration, close: async () => {} };
}

async function healthyRegistration(options: StartOptions): Promise<Registration | undefined> {
  try {
    return (await attach(options)).registration;
  } catch {
    return undefined;
  }
}

async function run(binary: string, args: string[]): Promise<void> {
  const { execFile } = await import("node:child_process");
  await new Promise<void>((resolve, reject) => {
    execFile(binary, args, (err, _stdout, stderr) => {
      if (err) reject(new Error(`\`${binary} ${args.join(" ")}\` failed: ${stderr.trim() || err.message}`, { cause: err }));
      else resolve();
    });
  });
}
