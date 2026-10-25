import type { Backoff, Connection } from "./connection.js";
import { CyberClientError, errorFromResponse } from "./errors.js";
import type { CallParams, OperationId, OperationSpec, RequestOptions } from "./generated/operations.js";
import { operations } from "./generated/operations.js";
import type { EventEnvelope } from "./generated/types.js";
import { uuidv7 } from "./ids.js";
import { sleep } from "./sleep.js";
import { readSse, type SseMessage } from "./sse.js";
import type { ToolRegistry } from "./tools.js";

/** How a client reaches a server: HTTP (`HttpTransport`) or JSON-RPC over stdio (`RpcTransport`). */
export interface Transport {
  /** The Location sent on Location-scoped requests. */
  readonly directory: string | undefined;
  readonly reconnect: Backoff;
  /** Application tools, shared by every Location of the connection. */
  readonly tools: ToolRegistry;
  call(id: OperationId, params: CallParams, options?: RequestOptions): Promise<unknown>;
  /** Open one event-stream connection; resolves once the server accepted it. */
  open(id: OperationId, params: CallParams, signal: AbortSignal): Promise<AsyncIterable<EventEnvelope>>;
  /** The same transport for another Location. */
  at(directory: string): Transport;
}

interface RawResponse {
  status: number;
  text: string;
}

/** HTTP for one Location: builds requests, retries transport failures and decodes bodies. */
export class HttpTransport implements Transport {
  constructor(
    readonly connection: Connection,
    readonly directory: string | undefined,
  ) {}

  get reconnect(): Backoff {
    return this.connection.reconnect;
  }

  get tools(): ToolRegistry {
    return this.connection.tools;
  }

  at(directory: string): Transport {
    return new HttpTransport(this.connection, directory);
  }

  async call(id: OperationId, params: CallParams, options: RequestOptions = {}): Promise<unknown> {
    const op: OperationSpec = operations[id];
    const base = await this.connection.baseUrl();
    if (op.tag === "hub") checkHubBaseUrl(base);
    const url = buildUrl(base, op, params);
    const headers = await this.headers(op, options.headers);
    const init: RequestInit = { method: op.method, headers, ...(op.tag === "hub" && { redirect: "error" }) };
    if (op.method !== "GET") headers.set("idempotency-key", options.idempotencyKey ?? uuidv7());
    if (params.body !== undefined) {
      headers.set("content-type", "application/json");
      init.body = JSON.stringify(params.body);
    }
    if (op.binary) return this.fetchBinary(url, init, options.signal);
    // The same init (and so the same Idempotency-Key) is reused by every retry.
    const response = await this.withRetry(() => this.fetchText(url, init, options.signal), options.signal);
    if (response.status < 200 || response.status >= 300) throw errorFromResponse(response.status, response.text);
    return unwrapBody(op, parseJson(op, response.text));
  }

  async open(id: OperationId, params: CallParams, signal: AbortSignal): Promise<AsyncIterable<EventEnvelope>> {
    const op: OperationSpec = operations[id];
    const base = await this.connection.baseUrl();
    if (op.tag === "hub") checkHubBaseUrl(base);
    const url = buildUrl(base, op, params);
    const headers = await this.headers(op);
    headers.set("accept", "text/event-stream");
    let response: Response;
    try {
      response = await this.connection.fetch(url, { method: "GET", headers, signal, ...(op.tag === "hub" && { redirect: "error" }) });
    } catch (err) {
      if (signal.aborted) throw signal.reason;
      throw new CyberClientError("Transport", `event stream failed: ${describe(err)}`, { cause: err });
    }
    if (!response.ok) throw errorFromResponse(response.status, await response.text().catch(() => ""));
    if (!response.body) throw new CyberClientError("MalformedResponse", "event stream response has no body");
    return sseEnvelopes(response.body);
  }

  private async headers(op: OperationSpec, extra: Record<string, string> = {}): Promise<Headers> {
    const headers = new Headers({ ...this.connection.headers, ...extra });
    const authorization = await this.connection.authorization();
    if (op.tag === "hub" && !authorization?.startsWith("Bearer ")) {
      // Hub metadata is public. A local owner password must never leave its
      // execution boundary when a client is pointed at a project hub.
      headers.delete("authorization");
      if (op.name !== "hello") throw new CyberClientError("Transport", "hub operations require a separate bearer-authenticated client", { permanent: true });
    } else if (authorization) headers.set("authorization", authorization);
    if (op.located && this.directory !== undefined) {
      headers.set("x-cyber-directory", encodeURIComponent(this.directory));
    }
    return headers;
  }

  private async fetchBinary(url: string, init: RequestInit, signal?: AbortSignal): Promise<Uint8Array> {
    const timeout = new AbortController();
    const timer = setTimeout(() => timeout.abort(), this.connection.timeoutMs);
    try {
      const response = await this.connection.fetch(url, { ...init, redirect: "error", cache: "no-store", signal: signal ? AbortSignal.any([signal, timeout.signal]) : timeout.signal });
      if (!response.ok) throw errorFromResponse(response.status, await response.text());
      if (response.headers.get("content-type")?.split(";")[0] !== "image/png" || !response.body) throw new CyberClientError("MalformedResponse", "expected private PNG");
      const reader = response.body.getReader(); const chunks: Uint8Array[] = []; let length = 0;
      try { for (;;) { const part = await reader.read(); if (part.done) break; length += part.value.length; if (length > 8 * 1024 * 1024) throw new CyberClientError("MalformedResponse", "private PNG exceeds 8 MiB"); chunks.push(part.value); } } finally { await reader.cancel().catch(() => {}); }
      const bytes = new Uint8Array(length); let offset = 0; for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; } return bytes;
    } finally { clearTimeout(timer); }
  }

  private async fetchText(url: string, init: RequestInit, signal: AbortSignal | undefined): Promise<RawResponse> {
    const timeout = new AbortController();
    const timer = setTimeout(() => timeout.abort(), this.connection.timeoutMs);
    try {
      const response = await this.connection.fetch(url, { ...init, signal: signal ? AbortSignal.any([signal, timeout.signal]) : timeout.signal });
      return { status: response.status, text: await response.text() };
    } catch (err) {
      if (signal?.aborted) throw signal.reason;
      if (timeout.signal.aborted) {
        throw new CyberClientError("Timeout", `request timed out after ${this.connection.timeoutMs} ms`, { cause: err });
      }
      throw new CyberClientError("Transport", `request failed: ${describe(err)}`, { cause: err });
    } finally {
      clearTimeout(timer);
    }
  }

  private async withRetry<T>(attempt: () => Promise<T>, signal: AbortSignal | undefined): Promise<T> {
    for (let n = 0; ; n++) {
      try {
        return await attempt();
      } catch (err) {
        if (n >= this.connection.retries || !isTransportFailure(err)) throw err;
        await sleep(this.connection.retryDelayMs * 2 ** n, signal);
      }
    }
  }
}

/** Hub credentials follow the same explicit transport boundary as Rust HubClient. */
function checkHubBaseUrl(base: string): void {
  const reject = () => new CyberClientError("Transport", "hub URL requires HTTPS outside literal loopback and cannot contain user information, a query or a fragment", { permanent: true });
  let url: URL;
  try { url = new URL(base); } catch { throw reject(); }
  const authority = /^[a-z][a-z\d+.-]*:\/\/([^/?#]*)/i.exec(base.trim())?.[1] ?? "";
  if (url.username || url.password || authority.includes("@") || base.includes("?") || base.includes("#")) throw reject();
  const ipv4 = /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(url.hostname) && url.hostname.split(".").every((octet) => Number(octet) <= 255);
  const loopback = ipv4 || url.hostname === "[::1]";
  if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) throw reject();
}

function isTransportFailure(err: unknown): boolean {
  return err instanceof CyberClientError && (err.reason === "Transport" || err.reason === "Timeout");
}

export function buildUrl(base: string, op: OperationSpec, params: CallParams): string {
  const path = op.path.replace(/\{(\w+)\}/g, (_, name: string) => {
    const value = params.path?.[name];
    if (value === undefined) throw new TypeError(`missing path parameter "${name}"`);
    return encodeURIComponent(value);
  });
  const url = new URL(base + path);
  for (const [key, value] of Object.entries(params.query ?? {})) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

function parseJson(op: OperationSpec, text: string): unknown {
  if (text === "") return undefined;
  try {
    return JSON.parse(text);
  } catch (err) {
    throw new CyberClientError("MalformedResponse", `invalid JSON from ${op.method} ${op.path}`, { cause: err });
  }
}

/** A success body as methods return it: `{ data }` envelopes unwrapped, empty bodies `undefined`. */
export function unwrapBody(op: OperationSpec, body: unknown): unknown {
  if (body === undefined || body === null || !op.unwrap) return body ?? undefined;
  if (typeof body !== "object" || !("data" in body)) {
    throw new CyberClientError("MalformedResponse", `missing { data } envelope from ${op.method} ${op.path}`);
  }
  return body.data;
}

async function* sseEnvelopes(body: ReadableStream<Uint8Array>): AsyncGenerator<EventEnvelope> {
  for await (const message of readSse(body)) yield toEnvelope(message);
}

function toEnvelope(message: SseMessage): EventEnvelope {
  try {
    const event = JSON.parse(message.data) as EventEnvelope;
    if (typeof event === "object" && event !== null && typeof event.type === "string") return event;
  } catch {
    // Reported below.
  }
  throw new CyberClientError("MalformedResponse", `invalid event envelope: ${message.data.slice(0, 200)}`);
}

function describe(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
