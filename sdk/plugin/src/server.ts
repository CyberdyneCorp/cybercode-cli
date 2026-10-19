// The plugin side of the host protocol: answers the core's requests for one plugin over a
// transport, and sends the plugin's own requests (`secrets/get`, `permission/ask`, `log`).
import { createInterface } from "node:readline";
import { parse } from "./schema.js";
import type { HookEvents, Plugin, ProviderChunk, ToolContext } from "./plugin.js";
import {
  ErrorCodes,
  type InitializeParams,
  type InitializeResult,
  type Message,
  PROTOCOL_VERSION,
  type RpcError,
  type ToolResult,
  type Transport,
} from "./protocol.js";

class ProtocolError extends Error {
  constructor(
    readonly code: number,
    message: string,
    readonly data?: unknown,
  ) {
    super(message);
  }
}

/** A running protocol server. */
export interface Server {
  /** Resolves when the core sent `shutdown` or closed the transport. */
  readonly closed: Promise<void>;
}

export function serve(plugin: Plugin, transport: Transport): Server {
  return new PluginServer(plugin, transport);
}

/** Serve on this process's stdin and stdout, exiting after `shutdown`. */
export function serveStdio(plugin: Plugin): Server {
  const lines = createInterface({ input: process.stdin });
  const listeners: ((line: string) => void)[] = [];
  const closers: (() => void)[] = [];
  lines.on("line", (line) => listeners.forEach((listener) => listener(line)));
  lines.on("close", () => closers.forEach((closer) => closer()));
  const server = serve(plugin, {
    send: (line) => void process.stdout.write(`${line}\n`),
    onLine: (listener) => void listeners.push(listener),
    onClose: (listener) => void closers.push(listener),
    close: () => lines.close(),
  });
  void server.closed.then(() => process.exit(0));
  return server;
}

class PluginServer implements Server {
  readonly closed: Promise<void>;
  private config: Record<string, unknown> = {};
  private readonly calls = new Map<string | number, AbortController>();
  private readonly waiting = new Map<string, (message: Message) => void>();
  private next = 1;
  private finish!: () => void;

  constructor(
    private readonly plugin: Plugin,
    private readonly transport: Transport,
  ) {
    this.closed = new Promise((resolve) => (this.finish = resolve));
    transport.onLine((line) => void this.receive(line));
    transport.onClose(() => this.finish());
  }

  private send(message: Omit<Message, "jsonrpc">): void {
    this.transport.send(JSON.stringify({ jsonrpc: "2.0", ...message }));
  }

  /** A request to the core, answered by its `id`. */
  private request(method: string, params: unknown): Promise<unknown> {
    const id = `plugin-${this.next++}`;
    return new Promise((resolve, reject) => {
      this.waiting.set(id, (message) => {
        if (message.error) reject(new Error(message.error.message));
        else resolve(message.result);
      });
      this.send({ id, method, params });
    });
  }

  private async receive(line: string): Promise<void> {
    let message: Message;
    try {
      message = JSON.parse(line) as Message;
    } catch {
      return;
    }
    if (message.method === undefined) {
      const waiter = message.id === undefined ? undefined : this.waiting.get(String(message.id));
      if (waiter) {
        this.waiting.delete(String(message.id));
        waiter(message);
      }
      return;
    }
    if (message.id === undefined) {
      this.notification(message.method, message.params);
      return;
    }
    const id = message.id;
    try {
      const result = await this.handle(id, message.method, message.params);
      this.send({ id, result: result ?? null });
    } catch (error) {
      const failure: RpcError =
        error instanceof ProtocolError
          ? { code: error.code, message: error.message, ...(error.data === undefined ? {} : { data: error.data }) }
          : { code: ErrorCodes.internal, message: error instanceof Error ? error.message : String(error) };
      this.send({ id, error: failure });
    }
    if (message.method === "shutdown") {
      this.transport.close();
      this.finish();
    }
  }

  private notification(method: string, params: unknown): void {
    if (method === "$/cancelRequest") {
      const id = (params as { id?: string | number } | undefined)?.id;
      if (id !== undefined) this.calls.get(id)?.abort();
    } else if (method === "config/changed") {
      this.config = ((params as { config?: Record<string, unknown> }).config ?? {}) as Record<string, unknown>;
      this.plugin.options.configChanged?.(this.config);
    }
  }

  private async handle(id: string | number, method: string, params: unknown): Promise<unknown> {
    switch (method) {
      case "initialize":
        return this.initialize(params as InitializeParams);
      case "tool/execute":
        return this.execute(id, params as ExecuteParams);
      case "provider/stream":
        return this.stream(id, params as StreamParams);
      case "shutdown":
        await this.plugin.options.shutdown?.();
        return null;
      default:
        if (method.startsWith("hook/")) return this.hook(method.slice("hook/".length), params);
        throw new ProtocolError(ErrorCodes.methodNotFound, `unknown method ${method}`);
    }
  }

  private async initialize(params: InitializeParams): Promise<InitializeResult> {
    if (params.protocolVersion !== PROTOCOL_VERSION) {
      throw new ProtocolError(ErrorCodes.invalidParams, `unsupported protocol version ${params.protocolVersion}`, {
        protocolVersion: PROTOCOL_VERSION,
      });
    }
    this.config = params.config ?? {};
    await this.plugin.options.setup?.(this.config);
    const { tools = {}, hooks = {}, providers = {} } = this.plugin.options;
    return {
      protocolVersion: PROTOCOL_VERSION,
      tools: Object.entries(tools).map(([name, tool]) => ({
        name,
        description: tool.description,
        inputSchema: tool.inputSchema,
        ...(tool.options.annotations ? { annotations: tool.options.annotations } : {}),
      })),
      hooks: Object.keys(hooks).map((event) => ({ event })),
      providers: Object.entries(providers).map(([id, adapter]) => ({ id, models: adapter.options.models })),
      catalogTransforms: [],
    };
  }

  private async execute(id: string | number, params: ExecuteParams): Promise<ToolResult> {
    const tool = this.plugin.options.tools?.[params.name];
    if (!tool) throw new ProtocolError(ErrorCodes.invalidParams, `unknown tool ${params.name}`);
    const controller = new AbortController();
    this.calls.set(id, controller);
    const context: ToolContext = {
      callId: params.callId ?? "",
      sessionId: params.sessionId ?? "",
      attempt: params.attempt ?? 1,
      operationKey: params.operationKey ?? "",
      signal: controller.signal,
      config: this.config,
      secret: async (key) => ((await this.request("secrets/get", { key })) as { value: string }).value,
      askPermission: async (resource, message) =>
        ((await this.request("permission/ask", { callId: params.callId, resource, message })) as { granted: boolean })
          .granted,
      log: (level, message) => this.send({ method: "log", params: { level, message } }),
    };
    try {
      const input = parse(tool.options.input, params.input, "input");
      const value = parse(tool.options.output, await tool.options.execute(input, context), "output");
      if (typeof value === "string") return { content: value };
      return { content: JSON.stringify(value ?? null), structured: value ?? null };
    } catch (error) {
      return { content: error instanceof Error ? error.message : String(error), isError: true };
    } finally {
      this.calls.delete(id);
    }
  }

  private async hook(event: string, params: unknown): Promise<unknown> {
    const handler = (this.plugin.options.hooks as Record<string, ((event: unknown) => unknown) | undefined> | undefined)?.[
      event
    ];
    if (!handler) throw new ProtocolError(ErrorCodes.methodNotFound, `no handler for hook ${event}`);
    return (await handler(params as HookEvents[keyof HookEvents])) ?? {};
  }

  /** Stream one provider answer as `provider/chunk` notifications, then settle. */
  private async stream(id: string | number, params: StreamParams): Promise<unknown> {
    const adapter = this.plugin.options.providers?.[params.provider];
    if (!adapter) throw new ProtocolError(ErrorCodes.invalidParams, `unknown provider ${params.provider}`);
    const controller = new AbortController();
    this.calls.set(id, controller);
    try {
      let chunks = 0;
      for await (const chunk of adapter.options.stream(params.request, controller.signal)) {
        this.send({ method: "provider/chunk", params: { id, chunk: chunk satisfies ProviderChunk } });
        chunks += 1;
      }
      return { done: true, chunks };
    } finally {
      this.calls.delete(id);
    }
  }
}

interface ExecuteParams {
  name: string;
  input: unknown;
  callId?: string;
  sessionId?: string;
  attempt?: number;
  operationKey?: string;
}

interface StreamParams {
  provider: string;
  request: { model: string; messages: unknown[]; tools?: unknown[] };
}
