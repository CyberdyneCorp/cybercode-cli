// @cyber-code/plugin: write Cyber Code plugins in TypeScript (`plugins-marketplace` →
// TypeScript plugin kit). A plugin's `main` exports `definePlugin({...})`; the core runs it
// in a sandboxed host process and speaks the JSON-RPC host protocol over stdio.
export {
  definePlugin,
  tool,
  provider,
  type HookDecision,
  type HookEvents,
  type HookHandler,
  type HookHandlers,
  type Plugin,
  type PluginOptions,
  type PluginTool,
  type ProviderAdapter,
  type ProviderChunk,
  type ProviderOptions,
  type ProviderRequest,
  type ToolContext,
  type ToolOptions,
} from "./plugin.js";
export { serve, serveStdio, type Server } from "./server.js";
export { createHarness, ProtocolFailure, type Harness, type HarnessOptions, type ToolCall } from "./harness.js";
export { type JsonSchema, type Schema, type SchemaLike } from "./schema.js";
export {
  PROTOCOL_VERSION,
  type InitializeParams,
  type InitializeResult,
  type ToolAnnotations,
  type ToolDeclaration,
  type ToolResult,
  type Transport,
} from "./protocol.js";
