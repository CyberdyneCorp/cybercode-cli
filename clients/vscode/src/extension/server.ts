/**
 * Finding, reaching and starting the local server (`editor-integration` → Extension server
 * connection, Extension server auto-start consent). Only `server.json` and the IDE token
 * file are read from the state directory; the owner password file never is.
 */
import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { Agent, fetch as undiciFetch } from "undici";
import { stateDir } from "@cyber-code/sdk";
import type { Registration, Transport } from "./transport.js";

/** Paths the extension read from the state directory, for tests. */
export const stateReads: string[] = [];

export async function cyberState(): Promise<string> {
  return stateDir();
}

export async function readStateFile(state: string, name: string): Promise<string | undefined> {
  const path = join(state, name);
  stateReads.push(path);
  try {
    return await readFile(path, "utf8");
  } catch {
    return undefined;
  }
}

export async function readRegistration(state: string): Promise<Registration | undefined> {
  const text = await readStateFile(state, "server.json");
  if (text === undefined) return undefined;
  try {
    const value = JSON.parse(text) as Registration;
    return typeof value === "object" && value !== null ? value : undefined;
  } catch {
    return undefined;
  }
}

/**
 * `fetch` bound to the chosen transport, and the base URL to use with it. Requests never go
 * through a proxy: loopback TCP and the socket use a private dispatcher.
 */
export function transportFetch(transport: Exclude<Transport, { kind: "refused" }>): { fetch: typeof fetch; baseUrl: string; close: () => Promise<void> } {
  const agent = transport.kind === "socket" ? new Agent({ connect: { socketPath: transport.path } }) : new Agent();
  const bound = ((input: Parameters<typeof fetch>[0], init?: RequestInit) =>
    undiciFetch(input as never, { ...(init as object), dispatcher: agent } as never)) as unknown as typeof fetch;
  const baseUrl = transport.kind === "socket" ? "http://localhost" : transport.url;
  return { fetch: bound, baseUrl, close: () => agent.close() };
}

/** `<executable> service start`, an argument vector without a shell, within 10 seconds. */
export function startServer(executable: string, cwd: string): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(executable, ["service", "start"], { cwd, timeout: 10_000, shell: false, windowsHide: true }, (error) => {
      if (error) reject(new Error("cyber service start failed"));
      else resolve();
    });
  });
}
