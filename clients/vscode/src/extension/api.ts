/**
 * The extension's server connection (`editor-integration` → Extension server connection,
 * Extension IDE token authentication). Every request leaves the extension host through an
 * injected `fetch`: a Unix-socket dispatcher or loopback TCP. The IDE token lives only in
 * the closure of one connection; logging records method, route template, status and
 * request ID, never headers, bodies or URLs with query strings.
 */
import { Cyber, CyberApiError, CyberClientError, type CyberClient } from "@cyber-code/sdk";
import type { Registration } from "./transport.js";

export const SCOPE_MESSAGE = "This action needs the Cyber CLI or dashboard";

export type Log = (line: string) => void;

/** `/api/v1/sessions/{id}/permissions/{id}/reply` from a concrete URL: no IDs, no query. */
export function routeTemplate(url: string): string {
  let path: string;
  try {
    path = new URL(url).pathname;
  } catch {
    return "<invalid>";
  }
  return path
    .split("/")
    .map((segment) => (/^[a-z]+_[A-Za-z0-9]+$/.test(segment) || /^[0-9a-f-]{16,}$/i.test(segment) ? "{id}" : segment))
    .join("/");
}

/** `fetch` that logs one redacted line per response or failure. */
export function loggingFetch(inner: typeof fetch, log: Log): typeof fetch {
  return async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const method = init?.method ?? "GET";
    try {
      const response = await inner(input, init);
      log(`${method} ${routeTemplate(url)} ${response.status} ${response.headers.get("x-request-id") ?? "-"}`);
      return response;
    } catch (error) {
      log(`${method} ${routeTemplate(url)} failed`);
      throw error;
    }
  };
}

export interface ClientOptions {
  baseUrl: string;
  fetch: typeof fetch;
  token: string;
  directory: string;
  log: Log;
}

/**
 * An SDK client authenticated with the IDE token only. The SDK never falls back to the
 * owner password file because `auth` is always given.
 */
export function createClient(options: ClientOptions): CyberClient {
  const token = options.token;
  return Cyber.connect({
    baseUrl: options.baseUrl,
    fetch: loggingFetch(options.fetch, options.log),
    auth: { type: "account", tokenProvider: () => token },
    directory: options.directory,
    retry: { retries: 2, delayMs: 250 },
  });
}

export type Failure = "unauthorized" | "scope" | "incompatible" | "transport" | "other";

export function classify(error: unknown): Failure {
  if (error instanceof CyberApiError) return classifyApi(error);
  if (error instanceof CyberClientError) {
    if (error.status === 401) return "unauthorized";
    if (error.status === 404) return "incompatible";
    return error.reason === "Transport" || error.reason === "Timeout" ? "transport" : "other";
  }
  return "other";
}

function classifyApi(error: CyberApiError): Failure {
  if (error.status === 401) return "unauthorized";
  const reason = (error.body as unknown as Record<string, unknown>).reason;
  if (error.status === 403 && reason === "ide_token_scope") return "scope";
  if (error.status === 503 && error.tag === "ServiceUnavailableError") return "incompatible";
  return "other";
}

/** A message for the panel; never includes request details. */
export function describe(error: unknown): string {
  const kind = classify(error);
  if (kind === "scope") return SCOPE_MESSAGE;
  if (kind === "unauthorized") return "The IDE token was refused. Create a new IDE token.";
  if (kind === "transport") return "Connection to the Cyber Code server was interrupted.";
  if (error instanceof CyberApiError) return error.body.message || error.tag;
  return "The request failed.";
}

export interface Health {
  apiVersion?: string;
  version?: string;
  build?: string;
  features?: string[];
}

export type Probe = { ok: true } | { ok: false; message: string };

function pidAlive(pid: number | undefined): boolean {
  if (!pid || !Number.isInteger(pid)) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

/**
 * Before any credential is sent: the unauthenticated health route must report API `v1`
 * and the `ide` group; over TCP, the registered pid must be alive and health must report
 * the registration's version and build.
 */
export async function preflight(fetcher: typeof fetch, baseUrl: string, registration: Registration, tcp: boolean): Promise<Probe> {
  if (tcp && !pidAlive(registration.pid)) return { ok: false, message: "The registered Cyber Code server is not running." };
  let health: Health;
  try {
    const response = await fetcher(`${baseUrl}/api/v1/health`, { signal: AbortSignal.timeout(5000) });
    health = (await response.json()) as Health;
  } catch {
    return { ok: false, message: "The Cyber Code server did not answer." };
  }
  if (health.apiVersion !== "v1" || !health.features?.includes("ide")) {
    return { ok: false, message: `The Cyber Code server ${health.version ?? "(unknown version)"} is not compatible with this extension; upgrade cyber.` };
  }
  if (tcp && (health.version !== registration.version || health.build !== registration.build)) {
    return { ok: false, message: "The server answering on the registered port is not the registered Cyber Code server." };
  }
  return { ok: true };
}

/** After the first authenticated call: the server must be the registered one. */
export async function confirmIdentity(client: CyberClient, registration: Registration): Promise<boolean> {
  const service = (await client.request("v1.service.get")) as { id?: string };
  return registration.id === undefined || service.id === registration.id;
}
