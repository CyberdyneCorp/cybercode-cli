import type { Backoff } from "./connection.js";
import { CyberApiError, CyberClientError } from "./errors.js";
import type { CallParams, OperationId, OperationSpec, RequestOptions } from "./generated/operations.js";
import { operations } from "./generated/operations.js";
import type { EventEnvelope } from "./generated/types.js";
import { uuidv7 } from "./ids.js";
import { AsyncQueue } from "./queue.js";
import type { ToolRegistry } from "./tools.js";
import { unwrapBody, type Transport } from "./transport.js";

/** Line-oriented I/O to a JSON-RPC peer, e.g. the stdin/stdout of `cyber serve --stdio`. */
export interface RpcIo {
  /** Send one line (without the trailing newline). */
  send(line: string): void;
  /** Lines from the peer; the channel closes when this ends. */
  lines: AsyncIterable<string>;
  /** Close the underlying connection (optional). */
  close?(): void;
}

/** Answers a request the peer sends us (e.g. `tool.execute`); a throw becomes a JSON-RPC error. */
export type RpcHandler = (params: unknown) => unknown;

interface RpcErrorObject {
  code: number;
  message: string;
  data?: unknown;
}

interface RpcMessage {
  id?: number | string | null;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: RpcErrorObject;
}

interface Pending {
  resolve(result: unknown): void;
  reject(err: unknown): void;
}

/**
 * A JSON-RPC 2.0 connection: requests with numeric IDs, `event` notifications routed to
 * subscriptions (buffered when they arrive before their subscription's result), and requests
 * from the peer answered by registered handlers.
 */
export class RpcChannel {
  /** Resolves when the connection is gone. */
  readonly closed: Promise<CyberClientError>;
  private resolveClosed!: (error: CyberClientError) => void;
  private readonly handlers = new Map<string, RpcHandler>();
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private readonly subscriptions = new Map<string, AsyncQueue<EventEnvelope>>();
  private readonly early = new Map<string, EventEnvelope[]>();
  private readonly ended = new Set<string>();
  private closedWith: CyberClientError | undefined;

  constructor(private readonly io: RpcIo) {
    this.closed = new Promise((resolve) => (this.resolveClosed = resolve));
    void this.read();
  }

  /** Answer the peer's requests for `method`. */
  handle(method: string, handler: RpcHandler): void {
    this.handlers.set(method, handler);
  }

  request(method: string, params: unknown, signal?: AbortSignal): Promise<unknown> {
    if (this.closedWith) return Promise.reject(this.closedWith);
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const onAbort = () => {
        this.pending.delete(id);
        reject(signal?.reason);
      };
      if (signal?.aborted) return onAbort();
      signal?.addEventListener("abort", onAbort, { once: true });
      const done = <T>(settle: (value: T) => void) => (value: T) => {
        signal?.removeEventListener("abort", onAbort);
        settle(value);
      };
      this.pending.set(id, { resolve: done(resolve), reject: done(reject) });
      this.io.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    });
  }

  /** Start a subscription; its events end when the returned iterable is closed or `signal` aborts. */
  async subscribe(method: string, params: unknown, signal: AbortSignal): Promise<AsyncIterable<EventEnvelope>> {
    const { subscription } = (await this.request(method, params, signal)) as { subscription: string };
    const queue = new AsyncQueue<EventEnvelope>();
    this.subscriptions.set(subscription, queue);
    for (const event of this.early.get(subscription) ?? []) queue.push(event);
    this.early.delete(subscription);
    const stop = () => queue.end();
    signal.addEventListener("abort", stop, { once: true });
    const unsubscribe = () => {
      signal.removeEventListener("abort", stop);
      this.subscriptions.delete(subscription);
      this.ended.add(subscription);
      this.request("v1.rpc.unsubscribe", { subscription }).catch(() => {});
    };
    return (async function* () {
      try {
        yield* queue;
      } finally {
        unsubscribe();
      }
    })();
  }

  /** Fail every pending request and subscription; later requests reject with `error`. */
  close(error: CyberClientError): void {
    if (this.closedWith) return;
    this.closedWith = error;
    this.io.close?.();
    this.resolveClosed(error);
    for (const pending of this.pending.values()) pending.reject(error);
    this.pending.clear();
    for (const queue of this.subscriptions.values()) queue.end(error);
    this.subscriptions.clear();
  }

  private async read(): Promise<void> {
    try {
      for await (const line of this.io.lines) {
        if (line.trim()) this.receive(line);
      }
      this.close(new CyberClientError("Transport", "the JSON-RPC peer closed the connection", { permanent: true }));
    } catch (err) {
      this.close(new CyberClientError("Transport", "the JSON-RPC connection failed", { cause: err, permanent: true }));
    }
  }

  private receive(line: string): void {
    let message: RpcMessage;
    try {
      message = JSON.parse(line) as RpcMessage;
    } catch {
      return; // Not a protocol frame (stray output); ignore it.
    }
    if (message.method === "event") return this.notify(message.params as EventParams | undefined);
    if (message.method !== undefined) return void this.answer(message.id, message.method, message.params);
    const pending = typeof message.id === "number" ? this.pending.get(message.id) : undefined;
    if (!pending || typeof message.id !== "number") return;
    this.pending.delete(message.id);
    if (message.error) pending.reject(rpcError(message.error));
    else pending.resolve(message.result);
  }

  /** Run the handler for a peer request and send its result or error. */
  private async answer(id: RpcMessage["id"], method: string, params: unknown): Promise<void> {
    const handler = this.handlers.get(method);
    let reply: Record<string, unknown>;
    try {
      if (!handler) throw new RpcMethodNotFound(method);
      reply = { result: (await handler(params)) ?? null };
    } catch (err) {
      const code = err instanceof RpcMethodNotFound ? -32601 : -32000;
      reply = { error: { code, message: err instanceof Error ? err.message : String(err) } };
    }
    if (id === undefined || id === null || this.closedWith) return; // A notification wants no reply.
    this.io.send(JSON.stringify({ jsonrpc: "2.0", id, ...reply }));
  }

  private notify(params: EventParams | undefined): void {
    const { subscription, event } = params ?? {};
    if (!subscription || !event || this.ended.has(subscription)) return;
    const queue = this.subscriptions.get(subscription);
    if (queue) queue.push(event);
    else this.early.set(subscription, [...(this.early.get(subscription) ?? []), event]);
  }
}

interface EventParams {
  subscription?: string;
  event?: EventEnvelope;
}

class RpcMethodNotFound extends Error {
  constructor(method: string) {
    super(`method not found: ${method}`);
  }
}

/** Map a JSON-RPC error: tagged bodies in `data` become `CyberApiError` with the HTTP status from the code. */
export function rpcError(error: RpcErrorObject): CyberApiError | CyberClientError {
  const data = error.data as { _tag?: unknown; message?: unknown } | null | undefined;
  const status = -32000 - error.code;
  if (typeof data?._tag === "string") {
    const httpStatus = status >= 100 && status <= 599 ? status : 400;
    return new CyberApiError(httpStatus, { ...data, _tag: data._tag, message: String(data.message ?? error.message) });
  }
  return new CyberClientError("UnexpectedStatus", `JSON-RPC error ${error.code}: ${error.message}`);
}

/** The client transport over an `RpcChannel` (embedded mode). */
export class RpcTransport implements Transport {
  constructor(
    private readonly channel: RpcChannel,
    readonly directory: string | undefined,
    readonly reconnect: Backoff,
    readonly tools: ToolRegistry,
  ) {}

  at(directory: string): Transport {
    return new RpcTransport(this.channel, directory, this.reconnect, this.tools);
  }

  async call(id: OperationId, params: CallParams, options: RequestOptions = {}): Promise<unknown> {
    const op: OperationSpec = operations[id];
    const idempotencyKey = op.method === "GET" ? undefined : (options.idempotencyKey ?? uuidv7());
    const result = await this.channel.request(id, this.params(op, params, idempotencyKey), options.signal);
    return unwrapBody(op, result);
  }

  open(id: OperationId, params: CallParams, signal: AbortSignal): Promise<AsyncIterable<EventEnvelope>> {
    return this.channel.subscribe(id, this.params(operations[id], params, undefined), signal);
  }

  private params(op: OperationSpec, params: CallParams, idempotencyKey: string | undefined): Record<string, unknown> {
    const query = Object.fromEntries(Object.entries(params.query ?? {}).filter(([, v]) => v !== undefined));
    return {
      ...(params.path && { path: params.path }),
      ...(Object.keys(query).length > 0 && { query }),
      ...(params.body !== undefined && { body: params.body }),
      ...(op.located && this.directory !== undefined && { directory: this.directory }),
      ...(idempotencyKey !== undefined && { idempotencyKey }),
    };
  }
}
