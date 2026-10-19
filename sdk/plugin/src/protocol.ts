// The plugin host protocol (`plugins-marketplace` → Host protocol): JSON-RPC 2.0 messages,
// one per line, between the Cyber Code core and a plugin host process.

/** The protocol version this kit speaks; the core refuses any other in `initialize`. */
export const PROTOCOL_VERSION = "1";

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type JsonObject = { [key: string]: JsonValue };

export interface RpcError {
  code: number;
  message: string;
  data?: unknown;
}

export interface Message {
  jsonrpc: "2.0";
  id?: string | number;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: RpcError;
}

/** A line-oriented duplex channel: stdio in a host process, memory in the harness. */
export interface Transport {
  send(line: string): void;
  onLine(listener: (line: string) => void): void;
  onClose(listener: () => void): void;
  close(): void;
}

/** What `initialize` carries from the core. */
export interface InitializeParams {
  protocolVersion: string;
  core?: { name: string; version: string };
  plugin?: { name: string; version: string; root: string };
  location?: { directory: string };
  capabilities?: Record<string, unknown>;
  config?: Record<string, unknown>;
}

/** One contributed tool as `initialize` declares it. */
export interface ToolDeclaration {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
  annotations?: ToolAnnotations;
}

/** MCP-style hints that drive the core's permission defaults. */
export interface ToolAnnotations {
  readOnlyHint?: boolean;
  destructiveHint?: boolean;
  idempotentHint?: boolean;
  title?: string;
}

export interface InitializeResult {
  protocolVersion: string;
  tools: ToolDeclaration[];
  hooks: { event: string }[];
  providers: { id: string; models: string[] }[];
  catalogTransforms: never[];
}

/** A `tool/execute` result: text content, an error flag and an optional typed value. */
export interface ToolResult {
  content: string;
  isError?: boolean;
  structured?: unknown;
}

export const ErrorCodes = {
  invalidParams: -32602,
  methodNotFound: -32601,
  internal: -32603,
} as const;
