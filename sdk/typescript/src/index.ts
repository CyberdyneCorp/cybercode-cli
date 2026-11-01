import { connect } from "./client.js";
import { start } from "./start.js";

/** Entry points: `Cyber.connect()` (no I/O until the first call) and `Cyber.start()`. */
export const Cyber = { connect, start };

export { CyberClient, type RequestParams, type SessionEventsOptions } from "./client.js";
export type { Auth, ConnectOptions } from "./connection.js";
export { CyberApiError, CyberClientError, type ClientErrorReason } from "./errors.js";
export * from "./generated/operations.js";
export type * from "./generated/types.js";
export { uuidv7 } from "./ids.js";
export { RpcChannel, RpcTransport, rpcError, type RpcIo } from "./rpc.js";
export type {
  AppTool,
  ClientHookHandler,
  ClientHookOptions,
  ClientHookResult,
  HookInput,
  ToolContext,
} from "./tools.js";
export { HttpTransport, type Transport } from "./transport.js";
export { readRegistration, readPassword, stateDir, type Registration } from "./local.js";
export type { PromptInput, PromptOptions, PromptResult, StopReason } from "./prompt.js";
export type {
  OnRequestOptions,
  PermissionDecision,
  PermissionHandler,
  PermissionRequest,
  QuestionHandler,
  QuestionRequest,
} from "./requests.js";
export type { MessageHistoryFilter, MessagingGroup } from "./messaging.js";
export { SseParser, type SseMessage } from "./sse.js";
export type { CyberHandle, StartOptions } from "./start.js";
export {
  WorkflowRunError,
  type ResultOptions,
  type WorkflowListFilter,
  type WorkflowRunHandle,
  type WorkflowRunInput,
} from "./workflows.js";

export type { CollaborationHandle, CollaborationStatus, CollaborationSnapshot, CollaborationMutationOptions, CollaborationWatchOptions, CollaborationQueuedReceipt, CollaborationMutationResult } from "./collaboration.js";
export { isCollaborationQueued } from "./collaboration.js";

export { byteOffset, type BinaryDocument } from "./binary.js";
