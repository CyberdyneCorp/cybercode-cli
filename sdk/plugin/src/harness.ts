// A test harness that plays the core's side of the host protocol in memory, so a plugin's
// tools, hooks and providers can be tested without starting Cyber Code.
import type { HookEvents, Plugin, ProviderChunk, ProviderRequest } from "./plugin.js";
import { type InitializeResult, type Message, PROTOCOL_VERSION, type ToolResult, type Transport } from "./protocol.js";
import { serve } from "./server.js";

export interface HarnessOptions {
  /** Settings passed in `initialize` (default `{}`). */
  config?: Record<string, unknown>;
  /** Secrets `secrets/get` answers; a missing key is an error. */
  secrets?: Record<string, string>;
  /** The answer to `permission/ask` (default: deny). */
  permission?: (resource: string, message?: string) => boolean;
  /** The protocol version the harness claims (default: this kit's). */
  protocolVersion?: string;
}

export interface ToolCall {
  callId?: string;
  sessionId?: string;
  signal?: AbortSignal;
}

export interface Harness {
  initialize(): Promise<InitializeResult>;
  callTool(name: string, input: unknown, call?: ToolCall): Promise<ToolResult>;
  hook<E extends keyof HookEvents>(event: E, payload: HookEvents[E]): Promise<unknown>;
  stream(provider: string, request: ProviderRequest): Promise<ProviderChunk[]>;
  configChanged(config: Record<string, unknown>): void;
  shutdown(): Promise<void>;
  /** Every `log` the plugin sent. */
  readonly logs: { level: string; message: string }[];
  /** Every request the plugin sent to the core, in order. */
  readonly requests: { method: string; params: unknown }[];
}

/** A pair of connected in-memory transports. */
function pipe(): [Transport, Transport] {
  const make = () => ({ lines: [] as ((line: string) => void)[], closes: [] as (() => void)[] });
  const [a, b] = [make(), make()];
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    [...a.closes, ...b.closes].forEach((listener) => listener());
  };
  const side = (self: typeof a, other: typeof a): Transport => ({
    send: (line) => queueMicrotask(() => other.lines.forEach((listener) => listener(line))),
    onLine: (listener) => void self.lines.push(listener),
    onClose: (listener) => void self.closes.push(listener),
    close,
  });
  return [side(a, b), side(b, a)];
}

export class ProtocolFailure extends Error {
  constructor(
    readonly code: number,
    message: string,
    readonly data?: unknown,
  ) {
    super(message);
  }
}

/** Drive `plugin` through the JSON-RPC protocol in memory. */
export function createHarness(plugin: Plugin, options: HarnessOptions = {}): Harness {
  const [core, host] = pipe();
  serve(plugin, host);
  const pending = new Map<number, (message: Message) => void>();
  const chunks = new Map<number, ProviderChunk[]>();
  const logs: { level: string; message: string }[] = [];
  const requests: { method: string; params: unknown }[] = [];
  let next = 1;

  const send = (message: Omit<Message, "jsonrpc">) => core.send(JSON.stringify({ jsonrpc: "2.0", ...message }));

  core.onLine((line) => {
    const message = JSON.parse(line) as Message;
    if (message.method === undefined) {
      const waiter = pending.get(Number(message.id));
      pending.delete(Number(message.id));
      waiter?.(message);
      return;
    }
    const params = message.params as Record<string, unknown> | undefined;
    if (message.method === "log") {
      logs.push({ level: String(params?.level), message: String(params?.message) });
      return;
    }
    if (message.method === "provider/chunk") {
      chunks.get(Number(params?.id))?.push(params?.chunk as ProviderChunk);
      return;
    }
    requests.push({ method: message.method, params });
    if (message.id === undefined) return;
    send({ id: message.id, ...answer(message.method, params, options) });
  });

  const request = (method: string, params: unknown, signal?: AbortSignal): Promise<unknown> => {
    const id = next++;
    return new Promise((resolve, reject) => {
      pending.set(id, (message) => {
        if (message.error) reject(new ProtocolFailure(message.error.code, message.error.message, message.error.data));
        else resolve(message.result);
      });
      signal?.addEventListener("abort", () => send({ method: "$/cancelRequest", params: { id } }), { once: true });
      send({ id, method, params });
    });
  };

  return {
    logs,
    requests,
    initialize: async () =>
      (await request("initialize", {
        protocolVersion: options.protocolVersion ?? PROTOCOL_VERSION,
        core: { name: "harness", version: "0.0.0" },
        plugin: { name: "harness-plugin", version: "0.0.0", root: "." },
        config: options.config ?? {},
      })) as InitializeResult,
    callTool: async (name, input, call = {}) =>
      (await request(
        "tool/execute",
        {
          name,
          input,
          callId: call.callId ?? `call_${next}`,
          sessionId: call.sessionId ?? "ses_harness",
          attempt: 1,
          operationKey: `ses_harness:${call.callId ?? next}`,
        },
        call.signal,
      )) as ToolResult,
    hook: (event, payload) => request(`hook/${event}`, payload),
    stream: async (provider, body) => {
      const id = next;
      chunks.set(id, []);
      await request("provider/stream", { provider, request: body });
      const out = chunks.get(id) ?? [];
      chunks.delete(id);
      return out;
    },
    configChanged: (config) => send({ method: "config/changed", params: { config } }),
    shutdown: async () => {
      await request("shutdown", {});
    },
  };
}

/** The core's answer to a plugin's request. */
function answer(
  method: string,
  params: Record<string, unknown> | undefined,
  options: HarnessOptions,
): Pick<Message, "result" | "error"> {
  if (method === "secrets/get") {
    const key = String(params?.key);
    const value = options.secrets?.[key];
    return value === undefined
      ? { error: { code: -32002, message: `no secret ${JSON.stringify(key)} is set` } }
      : { result: { value } };
  }
  if (method === "permission/ask") {
    const granted = options.permission?.(String(params?.resource), params?.message as string | undefined) ?? false;
    return { result: { granted } };
  }
  return { error: { code: -32601, message: `unknown method ${method}` } };
}
