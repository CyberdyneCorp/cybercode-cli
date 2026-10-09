import type { Backoff } from "./connection.js";
import { CyberClientError } from "./errors.js";
import { AsyncQueue } from "./queue.js";
import { RpcChannel, type RpcIo } from "./rpc.js";
import { sleep } from "./sleep.js";

const TOOL_NAME = /^[A-Za-z][A-Za-z0-9_-]{0,63}$/;

export interface ToolContext {
  sessionID: string;
  callID: string;
}

/** A tool that runs in this process and is offered to the model by the server. */
export interface AppTool<Input = any> {
  /** `^[A-Za-z][A-Za-z0-9_-]{0,63}$`, not a built-in tool name. */
  name: string;
  description: string;
  /** JSON Schema of the input object. */
  input: Record<string, unknown>;
  /** The output: a string is passed as is; other values are JSON-encoded by the server. A throw is reported to the model as the tool's failure. */
  execute(input: Input, context: ToolContext): unknown;
}

interface ExecuteParams {
  registration_id?: string;
  name?: string;
  input?: unknown;
  session_id?: string;
  call_id?: string;
}

/** A source of the current JSON-RPC channel; `attach` runs on every new connection. */
export interface ChannelHandle {
  current(): Promise<RpcChannel>;
  close(): void;
}

export type ChannelFactory = (attach: (channel: RpcChannel) => Promise<void>) => ChannelHandle;

/**
 * Application tools registered over a JSON-RPC channel. The channel is opened on the first
 * registration and closed after the last unregistration; tools are re-registered after a reconnect.
 */
export class ToolRegistry {
  private readonly tools = new Map<string, AppTool>();
  private readonly registrations = new WeakMap<RpcChannel, Map<string, { tool: AppTool; id: string }>>();
  private handle: ChannelHandle | undefined;

  constructor(
    private readonly factory: ChannelFactory,
    private readonly report: (err: unknown) => void = (err) => console.error("cyber: tool re-registration failed", err),
  ) {}

  /** Register a tool; resolves with a function that unregisters it. */
  async register(tool: AppTool): Promise<() => Promise<void>> {
    if (!TOOL_NAME.test(tool.name)) throw new TypeError(`invalid tool name "${tool.name}": must match ${TOOL_NAME}`);
    if (this.tools.has(tool.name)) throw new Error(`tool "${tool.name}" is already registered by this client`);
    this.handle ??= this.factory((channel) => this.attach(channel));
    try {
      const channel = await this.handle.current();
      await this.bind(channel, tool);
    } catch (err) {
      this.closeIfIdle();
      throw err;
    }
    this.tools.set(tool.name, tool);
    return () => this.unregister(tool);
  }

  /** With no tools left, close the channel: that drops server-side state and lets the process exit. */
  private closeIfIdle(): boolean {
    if (this.tools.size > 0 || !this.handle) return false;
    this.handle.close();
    this.handle = undefined;
    return true;
  }

  private async unregister(tool: AppTool): Promise<void> {
    if (this.tools.get(tool.name) !== tool) return;
    this.tools.delete(tool.name);
    if (!this.handle || this.closeIfIdle()) return;
    const channel = await this.handle.current();
    await channel.request("v1.tool.unregister", { name: tool.name });
  }

  /** Serve `tool.execute` on a new connection and restore the registrations it lost. */
  private async attach(channel: RpcChannel): Promise<void> {
    this.registrations.set(channel, new Map());
    channel.handle("tool.execute", (params) => this.execute(channel, params as ExecuteParams));
    for (const tool of this.tools.values()) {
      await this.bind(channel, tool).catch(this.report);
    }
  }

  private async bind(channel: RpcChannel, tool: AppTool): Promise<void> {
    const result = await channel.request("v1.tool.register", spec(tool)) as { registered?: string; registration_id?: string };
    if (result?.registered !== tool.name || typeof result.registration_id !== "string" || !result.registration_id.startsWith("reg_")) {
      throw new Error("server did not return a tool registration identity");
    }
    this.registrations.get(channel)!.set(tool.name, { tool, id: result.registration_id });
  }

  private async execute(channel: RpcChannel, params: ExecuteParams): Promise<unknown> {
    const name = params.name ?? "";
    const current = this.tools.get(name);
    if (!current) throw new Error(`tool "${params.name}" is not registered by this client`);
    const registration = this.registrations.get(channel)?.get(name);
    if (!registration || registration.tool !== current || registration.id !== params.registration_id) {
      throw new Error(`Stale tool call: ${name}`);
    }
    const tool = registration.tool;
    const output = await tool.execute(params.input ?? {}, { sessionID: params.session_id ?? "", callID: params.call_id ?? "" });
    return output ?? "";
  }
}

function spec(tool: AppTool): Record<string, unknown> {
  return { name: tool.name, description: tool.description, input: tool.input };
}

/** A fixed channel (embedded mode): attached once, never reconnected. */
export function fixedChannel(channel: RpcChannel): ChannelFactory {
  return (attach) => {
    const ready = attach(channel).then(() => channel);
    return { current: () => ready, close: () => {} };
  };
}

/**
 * A channel that redials after it drops, with exponential backoff. The first dial's failure is
 * returned to the caller; later dials retry until `close()`.
 */
export class ReconnectingChannel implements ChannelHandle {
  private channel: Promise<RpcChannel> | undefined;
  private closed = false;
  private readonly stop = new AbortController();

  constructor(
    private readonly dial: () => Promise<RpcIo>,
    private readonly backoff: Backoff,
    private readonly attach: (channel: RpcChannel) => Promise<void>,
  ) {}

  current(): Promise<RpcChannel> {
    if (this.closed) return Promise.reject(new CyberClientError("Transport", "the tool channel is closed"));
    this.channel ??= this.connect().catch((err: unknown) => {
      this.channel = undefined;
      throw err;
    });
    return this.channel;
  }

  close(): void {
    this.closed = true;
    this.stop.abort();
    void this.channel?.then((channel) => channel.close(new CyberClientError("Transport", "closed")), () => {});
  }

  private async connect(): Promise<RpcChannel> {
    const channel = new RpcChannel(await this.dial());
    void channel.closed.then(() => this.redial());
    await this.attach(channel);
    return channel;
  }

  private async redial(): Promise<void> {
    if (this.closed) return;
    let delay = this.backoff.initialMs;
    this.channel = new Promise<RpcChannel>((resolve) => {
      const attempt = async (): Promise<void> => {
        while (!this.closed) {
          await sleep(delay, this.stop.signal);
          delay = Math.min(delay * 2, this.backoff.maxMs);
          if (this.closed) return;
          try {
            return resolve(await this.connect());
          } catch {
            // Keep backing off.
          }
        }
      };
      void attempt();
    });
  }
}

/** Open a WebSocket as line I/O. Uses the global `WebSocket` (Node >= 22) unless one is given. */
export async function dialWebSocket(url: string, Socket: typeof WebSocket = globalThis.WebSocket): Promise<RpcIo> {
  if (!Socket) throw new CyberClientError("Transport", "no WebSocket implementation; pass `webSocket` to Cyber.connect()", { permanent: true });
  const socket = new Socket(url);
  const lines = new AsyncQueue<string>();
  socket.onmessage = (event) => lines.push(String(event.data));
  socket.onclose = () => lines.end();
  await new Promise<void>((resolve, reject) => {
    socket.onopen = () => resolve();
    socket.onerror = () => reject(new CyberClientError("Transport", `cannot open ${url.replace(/auth_token=[^&]*/, "auth_token=…")}`));
  });
  socket.onerror = () => {}; // `close` follows and ends the line queue.
  return { send: (line) => socket.send(line), lines, close: () => socket.close() };
}
