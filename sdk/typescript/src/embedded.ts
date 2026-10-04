import type { ChildProcessByStdio } from "node:child_process";
import type { Readable, Writable } from "node:stream";
import { CyberClient } from "./client.js";
import { Connection, type ConnectOptions } from "./connection.js";
import { CyberClientError } from "./errors.js";
import { RpcChannel, RpcTransport } from "./rpc.js";
import { fixedChannel, ToolRegistry } from "./tools.js";

export interface EmbeddedOptions extends Omit<ConnectOptions, "baseUrl" | "auth" | "fetch"> {
  /** The `cyber` executable (default: `cyber` on `PATH`). */
  binary?: string;
  /** Database file for the private server (sets `CYBER_DB`); in-memory when omitted. */
  db?: string;
  signal?: AbortSignal;
}

export interface EmbeddedServer {
  client: CyberClient;
  /** Close stdin so the server exits; kill it if it has not exited after 2 s. */
  close(): Promise<void>;
}

const EXIT_GRACE_MS = 2_000;

type ServerProcess = ChildProcessByStdio<Writable, Readable, null>;

/** Spawn `cyber serve --stdio` and talk JSON-RPC to it over stdin/stdout. */
export async function startEmbedded(options: EmbeddedOptions): Promise<EmbeddedServer> {
  const child = await spawnServer(options);
  const { createInterface } = await import("node:readline");
  const channel = new RpcChannel({
    send: (line) => void child.stdin.write(`${line}\n`),
    lines: createInterface({ input: child.stdout, crlfDelay: Infinity }),
  });
  child.once("exit", (code, signal) => {
    const how = signal ? `signal ${signal}` : `code ${code}`;
    channel.close(new CyberClientError("Transport", `the embedded server exited (${how})`, { permanent: true }));
  });
  const connection = new Connection(options);
  const tools = new ToolRegistry(fixedChannel(channel));
  const client = new CyberClient(new RpcTransport(channel, options.directory, connection.reconnect, tools));
  const close = once(() => stop(child));
  options.signal?.addEventListener("abort", () => void close(), { once: true });
  try {
    await client.health.get({ signal: options.signal });
  } catch (err) {
    await close();
    throw err;
  }
  return { client, close };
}

async function spawnServer(options: EmbeddedOptions): Promise<ServerProcess> {
  const { spawn } = await import("node:child_process");
  const env = { ...process.env, ...(options.db !== undefined && { CYBER_DB: options.db }) };
  const binary = options.binary ?? "cyber";
  const child = spawn(binary, ["serve", "--stdio"], { env, cwd: options.directory, stdio: ["pipe", "pipe", "inherit"] });
  child.stdin.on("error", () => {}); // EPIPE after exit surfaces through the `exit` event instead.
  await new Promise<void>((resolve, reject) => {
    child.once("spawn", resolve);
    child.once("error", (err) => reject(new Error(`cannot start \`${binary} serve --stdio\`: ${err.message}`, { cause: err })));
  });
  return child;
}

async function stop(child: ServerProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
  child.stdin.end();
  const timer = setTimeout(() => child.kill("SIGKILL"), EXIT_GRACE_MS);
  await exited;
  clearTimeout(timer);
}

export function once(fn: () => Promise<void>): () => Promise<void> {
  let result: Promise<void> | undefined;
  return () => (result ??= fn());
}
