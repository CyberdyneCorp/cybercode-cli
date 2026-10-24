//! Browser transport: explicit owner headers, page-memory credentials and inert data.
import { SseParser } from "../../sdk/typescript/src/sse.js";
import type { EventEnvelope } from "../../sdk/typescript/src/generated/types.js";

export class ApiFault extends Error {
  constructor(readonly kind: "auth" | "network" | "refused" | "missing" | "changed", readonly status = 0) {
    super(kind === "auth" ? "Sign in again to continue." : kind === "network" ? "Connection interrupted. The result may be unknown." : kind === "changed" ? "This item changed or was already answered." : kind === "missing" ? "This server does not offer this view." : "Server policy refused this action.");
  }
}
export interface RequestOptions {
  method?: "GET" | "POST" | "PUT" | "DELETE";
  body?: unknown;
  key?: string;
  directory?: string;
  signal?: AbortSignal;
  finishOnLogout?: boolean;
}
export interface StreamOptions { directory: string; signal: AbortSignal; opened: () => void }
export interface Port {
  password(value: string): void;
  clear(): void;
  request<T>(path: string, options?: RequestOptions): Promise<T>;
  stream(path: string, options: StreamOptions): AsyncIterable<EventEnvelope>;
}
export const operationKey = () => crypto.randomUUID();

export class HttpApi implements Port {
  #authorization: string | undefined;
  #controllers = new Set<AbortController>();
  #epoch = 0;
  readonly origin: string;
  constructor(origin: string, readonly unauthorized: () => void, readonly fetcher: typeof fetch = (input, init) => globalThis.fetch(input, init)) {
    this.origin = new URL(origin).origin;
  }
  password(value: string): void {
    this.clear();
    const bytes = new TextEncoder().encode(`cyber:${value}`);
    this.#authorization = `Basic ${btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(""))}`;
  }
  clear(): void {
    this.#epoch++;
    this.#authorization = undefined;
    for (const controller of this.#controllers) controller.abort();
    this.#controllers.clear();
  }
  async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    const controller = new AbortController();
    if (!options.finishOnLogout) this.#controllers.add(controller);
    const epoch = this.#epoch;
    const timer = setTimeout(() => controller.abort(), options.finishOnLogout ? 3000 : 30_000);
    try {
      const response = await this.fetcher(this.url(path), this.init(options, controller.signal));
      this.status(response.status, epoch);
      if (response.status === 204) return undefined as T;
      const text = await boundedText(response, 16 * 1024 * 1024);
      try { return JSON.parse(text) as T; } catch { throw new ApiFault("network"); }
    } catch (error) {
      if (error instanceof ApiFault) throw error;
      throw new ApiFault("network");
    } finally { clearTimeout(timer); this.#controllers.delete(controller); }
  }
  async *stream(path: string, options: StreamOptions): AsyncIterable<EventEnvelope> {
    const controller = new AbortController();
    this.#controllers.add(controller);
    const epoch = this.#epoch;
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    try {
      const init = this.init({ directory: options.directory, signal: options.signal }, controller.signal);
      (init.headers as Headers).set("accept", "text/event-stream");
      const response = await this.fetcher(this.url(path), init);
      this.status(response.status, epoch);
      if (!response.headers.get("content-type")?.includes("text/event-stream") || !response.body) throw new ApiFault("network");
      options.opened();
      reader = response.body.getReader();
      const decoder = new TextDecoder();
      const parser = new SseParser();
      let waitingBytes = 0;
      for (;;) {
        const chunk = await reader.read();
        if (chunk.done) throw new ApiFault("network");
        waitingBytes += chunk.value.byteLength;
        if (waitingBytes > 4 * 1024 * 1024) throw new ApiFault("network");
        const events = parser.push(decoder.decode(chunk.value, { stream: true }));
        if (events.length) waitingBytes = 0;
        for (const frame of events) {
          try {
            const event = JSON.parse(frame.data) as EventEnvelope;
            if (typeof event.type === "string" && typeof event.id === "string") yield event;
          } catch { throw new ApiFault("network"); }
        }
      }
    } catch (error) {
      if (error instanceof ApiFault) throw error;
      throw new ApiFault("network");
    } finally { await reader?.cancel().catch(() => {}); this.#controllers.delete(controller); }
  }
  private url(path: string): string {
    if (!path.startsWith("/") || path.startsWith("//") || path.includes("#")) throw new ApiFault("refused");
    const url = new URL(`/api/v1${path}`, this.origin);
    if (url.origin !== this.origin || !url.pathname.startsWith("/api/v1/")) throw new ApiFault("refused");
    return url.toString();
  }
  private init(options: RequestOptions, signal: AbortSignal): RequestInit {
    if (!this.#authorization) throw new ApiFault("auth", 401);
    const headers = new Headers({ authorization: this.#authorization, "x-cyber-client": "web", accept: "application/json" });
    if (options.directory !== undefined) headers.set("x-cyber-directory", encodeURIComponent(options.directory));
    const method = options.method ?? "GET";
    if (method !== "GET") headers.set("idempotency-key", options.key ?? operationKey());
    if (options.body !== undefined) headers.set("content-type", "application/json");
    return { method, headers, ...(options.finishOnLogout && { keepalive: true }), credentials: "omit", redirect: "error", cache: "no-store", signal: options.signal ? AbortSignal.any([signal, options.signal]) : signal, ...(options.body !== undefined && { body: JSON.stringify(options.body) }) };
  }
  private status(status: number, epoch: number): void {
    if (status === 401) { if (epoch === this.#epoch) { this.clear(); this.unauthorized(); } throw new ApiFault("auth", status); }
    if (status === 403) throw new ApiFault("refused", status);
    if (status === 404 || status === 405 || status === 501) throw new ApiFault("missing", status);
    if (status === 409) throw new ApiFault("changed", status);
    if (status < 200 || status >= 300) throw new ApiFault(status >= 500 ? "network" : "refused", status);
  }
}
async function boundedText(response: Response, maximum: number): Promise<string> {
  if (!response.body) return "";
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let result = "";
  let bytes = 0;
  try {
    for (;;) {
      const chunk = await reader.read();
      if (chunk.done) return result + decoder.decode();
      bytes += chunk.value.byteLength;
      if (bytes > maximum) throw new ApiFault("network");
      result += decoder.decode(chunk.value, { stream: true });
    }
  } finally { await reader.cancel().catch(() => {}); }
}
