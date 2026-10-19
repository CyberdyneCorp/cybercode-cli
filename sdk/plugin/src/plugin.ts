// `definePlugin`, `tool`, typed hook handlers and the provider adapter helper.
import { type JsonSchema, type Schema, toJsonSchema } from "./schema.js";
import type { ToolAnnotations } from "./protocol.js";

/** What a running tool call can use. */
export interface ToolContext {
  readonly callId: string;
  readonly sessionId: string;
  readonly attempt: number;
  /** Stable across retries of one call, for idempotent external APIs. */
  readonly operationKey: string;
  /** Aborted when the core cancels the call. */
  readonly signal: AbortSignal;
  /** The plugin's validated, non-secret `userConfig` settings. */
  readonly config: Readonly<Record<string, unknown>>;
  /** A secret setting; needs the `secrets` capability. */
  secret(key: string): Promise<string>;
  /** Ask the user to allow something during this call; an unattended run answers no. */
  askPermission(resource: string, message?: string): Promise<boolean>;
  log(level: "error" | "warn" | "info" | "debug", message: string): void;
}

export interface ToolOptions<I, O> {
  description: string;
  /** JSON Schema for the input object, or a Zod-style schema. */
  input: Schema<I>;
  /** The advertised JSON Schema when `input` cannot produce one itself. */
  jsonSchema?: JsonSchema;
  /** Validates the result when it is a Zod-style schema. */
  output?: Schema<O>;
  annotations?: ToolAnnotations;
  execute(input: I, context: ToolContext): O | Promise<O>;
}

/** A tool ready to register under a name in `definePlugin({ tools })`. */
export interface PluginTool<I = unknown, O = unknown> {
  readonly kind: "tool";
  readonly description: string;
  readonly inputSchema: JsonSchema;
  readonly options: ToolOptions<I, O>;
}

export function tool<I, O>(options: ToolOptions<I, O>): PluginTool<I, O> {
  const inputSchema = toJsonSchema(options.input as Schema, options.jsonSchema);
  if (inputSchema.type !== "object") throw new TypeError("a tool input schema must describe an object");
  return { kind: "tool", description: options.description, inputSchema, options };
}

/** The hook events a plugin can handle, with their payloads. */
export interface HookEvents {
  PreToolUse: { tool_name: string; tool_input: unknown; session_id: string };
  PostToolUse: { tool_name: string; tool_input: unknown; tool_response: unknown; session_id: string };
  UserPromptSubmit: { prompt: string; session_id: string };
  SessionStart: { source: string; session_id: string };
  Stop: { session_id: string };
}

/** A hook's decision, in the shape configured hooks return. */
export interface HookDecision {
  decision?: "allow" | "deny" | "ask" | "block";
  reason?: string;
  additional_context?: string;
  updated_input?: unknown;
  continue?: boolean;
  stop_reason?: string;
}

export type HookHandler<E extends keyof HookEvents> = (
  event: HookEvents[E],
) => HookDecision | void | Promise<HookDecision | void>;

export type HookHandlers = { [E in keyof HookEvents]?: HookHandler<E> };

/** One streamed piece of a provider answer. */
export type ProviderChunk =
  | { type: "text"; text: string }
  | { type: "tool_call"; id: string; name: string; arguments: string }
  | { type: "usage"; input_tokens: number; output_tokens: number };

export interface ProviderRequest {
  model: string;
  messages: unknown[];
  tools?: unknown[];
}

export interface ProviderOptions {
  /** Model IDs this provider serves. */
  models: string[];
  stream(request: ProviderRequest, signal: AbortSignal): AsyncIterable<ProviderChunk>;
}

export interface ProviderAdapter {
  readonly kind: "provider";
  readonly options: ProviderOptions;
}

/** A provider adapter, streamed over `provider/stream`. */
export function provider(options: ProviderOptions): ProviderAdapter {
  return { kind: "provider", options };
}

export interface PluginOptions {
  tools?: Record<string, PluginTool<any, any>>;
  hooks?: HookHandlers;
  providers?: Record<string, ProviderAdapter>;
  /** Called after `initialize` with the validated settings. */
  setup?(config: Readonly<Record<string, unknown>>): void | Promise<void>;
  /** Called on `config/changed`. */
  configChanged?(config: Readonly<Record<string, unknown>>): void;
  /** Called on `shutdown`, before the process exits. */
  shutdown?(): void | Promise<void>;
}

export interface Plugin {
  readonly kind: "plugin";
  readonly options: PluginOptions;
}

const NAME = /^[A-Za-z][A-Za-z0-9_-]{0,63}$/;
let autoServed = false;

/**
 * A plugin. When the module runs as a plugin host (the core sets `CYBER_PLUGIN_NAME`), it
 * starts serving the host protocol on stdio, so `export default definePlugin({...})` is a
 * complete `main`.
 */
export function definePlugin(options: PluginOptions): Plugin {
  for (const name of Object.keys(options.tools ?? {})) {
    if (!NAME.test(name) || name.startsWith("mcp__")) throw new TypeError(`invalid tool name ${JSON.stringify(name)}`);
  }
  const plugin: Plugin = { kind: "plugin", options };
  if (typeof process !== "undefined" && process.env.CYBER_PLUGIN_NAME && !autoServed) {
    autoServed = true;
    setImmediate(() => {
      void import("./server.js").then(({ serveStdio }) => serveStdio(plugin));
    });
  }
  return plugin;
}
