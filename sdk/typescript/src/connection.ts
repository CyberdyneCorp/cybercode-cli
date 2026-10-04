import { CyberClientError } from "./errors.js";
import type { RpcIo } from "./rpc.js";
import { readPassword, readRegistration } from "./local.js";
import { dialWebSocket, ReconnectingChannel, ToolRegistry } from "./tools.js";

export type Auth =
  /** HTTP Basic as user `cyber`. */
  | { type: "password"; password: string }
  /** A Bearer account token, fixed or fetched from `tokenProvider` before every request. */
  | { type: "account"; token?: string; tokenProvider?: () => string | Promise<string> };

export interface ConnectOptions {
  /** Server origin, e.g. `http://127.0.0.1:4747`. Defaults to the registered server (`server.json`). */
  baseUrl?: string;
  /** Defaults to the local password file when it is readable. */
  auth?: Auth;
  fetch?: typeof fetch;
  /** Extra headers sent on every request. */
  headers?: Record<string, string>;
  /** The Location sent as `x-cyber-directory` on Location-scoped requests. */
  directory?: string;
  /** Per-request timeout for non-streaming calls (default 60 000 ms). */
  timeoutMs?: number;
  /** Transport-failure retries for calls (default 3 retries, 250 ms doubling delay). */
  retry?: { retries?: number; delayMs?: number };
  /** Stream and tool-channel reconnect backoff (default 500 ms doubling up to 15 000 ms). */
  reconnect?: { initialMs?: number; maxMs?: number };
  /** WebSocket implementation for `client.tools.register` (default: the global `WebSocket`, Node >= 22). */
  webSocket?: typeof WebSocket;
}

export interface Backoff {
  initialMs: number;
  maxMs: number;
}

/** Settings shared by a client and every client scoped from it with `at()`. */
export class Connection {
  readonly fetch: typeof fetch;
  readonly headers: Record<string, string>;
  readonly timeoutMs: number;
  readonly retries: number;
  readonly retryDelayMs: number;
  readonly reconnect: Backoff;
  /** Application tools, over a lazily opened `/api/v1/ws` JSON-RPC channel. */
  readonly tools: ToolRegistry;
  private base: Promise<string> | undefined;
  private localPassword: Promise<string | undefined> | undefined;

  constructor(private readonly options: ConnectOptions) {
    this.fetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
    this.headers = { ...options.headers };
    this.timeoutMs = options.timeoutMs ?? 60_000;
    this.retries = options.retry?.retries ?? 3;
    this.retryDelayMs = options.retry?.delayMs ?? 250;
    this.reconnect = { initialMs: options.reconnect?.initialMs ?? 500, maxMs: options.reconnect?.maxMs ?? 15_000 };
    this.tools = new ToolRegistry((attach) => new ReconnectingChannel(() => this.dialTools(), this.reconnect, attach));
  }

  /** `ws(s)://…/api/v1/ws`, authenticated with `?auth_token=` since WebSockets cannot set headers. */
  private async dialTools(): Promise<RpcIo> {
    const url = new URL(`${await this.baseUrl()}/api/v1/ws`);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    const authorization = await this.authorization();
    const token = authorization?.replace(/^(Basic|Bearer) /, "");
    if (token) url.searchParams.set("auth_token", token);
    return dialWebSocket(url.toString(), this.options.webSocket);
  }

  /** The server origin, read from `server.json` on first use when no `baseUrl` was given. */
  baseUrl(): Promise<string> {
    this.base ??= this.resolveBaseUrl().catch((err: unknown) => {
      this.base = undefined;
      throw err;
    });
    return this.base;
  }

  /** The `authorization` header value, if any. */
  async authorization(): Promise<string | undefined> {
    const auth = this.options.auth;
    if (auth?.type === "password") return basic(auth.password);
    if (auth?.type === "account") return `Bearer ${await (auth.tokenProvider?.() ?? auth.token ?? "")}`;
    this.localPassword ??= readPassword();
    const password = await this.localPassword;
    return password === undefined ? undefined : basic(password);
  }

  private async resolveBaseUrl(): Promise<string> {
    const url = this.options.baseUrl ?? (await readRegistration())?.url;
    if (!url) {
      throw new CyberClientError("Transport", "no registered Cyber server; run `cyber service start` or pass `baseUrl`");
    }
    return url.replace(/\/+$/, "");
  }
}

function basic(password: string): string {
  return `Basic ${btoa(`cyber:${password}`)}`;
}
