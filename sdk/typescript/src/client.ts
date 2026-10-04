import { Connection, type ConnectOptions } from "./connection.js";
import {
  createGroups,
  operations,
  type CallParams,
  type Caller,
  type Groups,
  type OperationId,
  type Operations,
  type QueryValue,
  type RequestOptions,
  type StreamOperationId,
  type StreamOptions,
} from "./generated/operations.js";
import type { EventEnvelope, Receipt } from "./generated/types.js";
import { promptAndWait, switchSession, toParts, type PromptInput, type PromptOptions, type PromptResult } from "./prompt.js";
import {
  onPermissionRequest,
  onQuestionRequest,
  type OnRequestOptions,
  type PermissionHandler,
  type QuestionHandler,
} from "./requests.js";
import { eventStream } from "./stream.js";
import type { AppTool } from "./tools.js";
import { HttpTransport, type Transport } from "./transport.js";

/** Typed parameters of `client.request(id, params)`. */
export type RequestParams<K extends OperationId> = {
  path?: Operations[K]["path"];
  query?: Operations[K]["query"];
  body?: Operations[K]["body"];
};

export interface SessionEventsOptions extends StreamOptions {
  /** Replay durable events after this seq (default: the whole history). */
  after?: number;
}

export interface CyberClient extends Groups {}

/**
 * A client for one Location. Generated methods live in per-tag groups (`client.session.list()`,
 * `client.message.get()`, ...); the plural groups (`sessions`, `events`, `permissions`,
 * `questions`) hold hand-written helpers.
 */
export class CyberClient implements Caller {
  readonly #transport: Transport;

  readonly sessions = {
    /** Admit a prompt; with `wait: true`, resolve when the Session next becomes idle. */
    prompt: ((id: string, input: PromptInput, options: PromptOptions = {}) =>
      this.#prompt(id, input, options)) as PromptFn,
    /** Durable events of a Session: replays after `after`, then follows, resuming across reconnects. */
    events: (id: string, options: SessionEventsOptions = {}): AsyncIterableIterator<EventEnvelope> =>
      this.stream("v1.session.events", { path: { sessionID: id }, query: { after: options.after } }, options),
  };

  readonly events = {
    /** Live events of this client's Location (`scope: "all"` for every Location), reconnecting on failure. */
    subscribe: (options: StreamOptions & { scope?: "all" } = {}): AsyncIterableIterator<EventEnvelope> =>
      this.stream("v1.event.subscribe", { query: { scope: options.scope } }, options),
  };

  readonly tools = {
    /**
     * Register a tool that runs in this process; resolves with an unregister function.
     * Registrations last while the JSON-RPC channel is connected and are restored after a reconnect.
     */
    register: (tool: AppTool): Promise<() => Promise<void>> => this.#transport.tools.register(tool),
  };

  readonly permissions = {
    /** Reply to permission requests with `handler`'s decision; returns an unsubscribe function. */
    onRequest: (handler: PermissionHandler, options?: OnRequestOptions): (() => void) => onPermissionRequest(this, handler, options),
  };

  readonly questions = {
    /** Answer questions with `handler`'s result; returns an unsubscribe function. */
    onRequest: (handler: QuestionHandler, options?: OnRequestOptions): (() => void) => onQuestionRequest(this, handler, options),
  };

  constructor(transport: Transport) {
    this.#transport = transport;
    Object.assign(this, createGroups(this));
  }

  /** The Location sent as `x-cyber-directory`, if any. */
  get directory(): string | undefined {
    return this.#transport.directory;
  }

  /** A client scoped to `directory`; this client is unchanged. */
  at(directory: string): CyberClient {
    return new CyberClient(this.#transport.at(directory));
  }

  /** Call any operation by its OpenAPI operation ID. */
  request<K extends OperationId>(id: K, params: RequestParams<K> = {}, options?: RequestOptions): Promise<Operations[K]["response"]> {
    return this.call(id, params as CallParams, options);
  }

  call<K extends OperationId>(id: K, params: CallParams, options?: RequestOptions): Promise<Operations[K]["response"]> {
    return this.#transport.call(id, params, options) as Promise<Operations[K]["response"]>;
  }

  stream(id: StreamOperationId, params: CallParams, options: StreamOptions = {}): AsyncIterableIterator<EventEnvelope> {
    const resume = operations[id].query.some((name) => name === "after");
    return eventStream({
      open: (after, signal) => this.#transport.open(id, after === undefined ? params : withQuery(params, { after }), signal),
      resume,
      after: toSeq(params.query?.after),
      backoff: this.#transport.reconnect,
      signal: options.signal,
    });
  }

  async #prompt(id: string, input: PromptInput, options: PromptOptions): Promise<Receipt | PromptResult> {
    await switchSession(this, id, options);
    const parts = toParts(input);
    if (options.wait) return promptAndWait(this, id, parts, options);
    return this.session.prompt(id, { parts, delivery: options.delivery ?? "steer" }, options);
  }
}

interface PromptFn {
  (id: string, input: PromptInput, options: PromptOptions & { wait: true }): Promise<PromptResult>;
  (id: string, input: PromptInput, options?: PromptOptions & { wait?: false }): Promise<Receipt>;
  (id: string, input: PromptInput, options?: PromptOptions): Promise<Receipt | PromptResult>;
}

function withQuery(params: CallParams, query: Record<string, QueryValue>): CallParams {
  return { ...params, query: { ...params.query, ...query } };
}

function toSeq(value: QueryValue | undefined): number | undefined {
  return value === undefined ? undefined : Number(value);
}

/** Construct a client. No network I/O happens until the first request. */
export function connect(options: ConnectOptions = {}): CyberClient {
  return new CyberClient(new HttpTransport(new Connection(options), options.directory));
}
