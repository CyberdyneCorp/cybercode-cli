import type { Groups, RequestOptions } from "./generated/operations.js";
import type { HubAgent, HubCreateTask, HubMember, HubMessage, HubRepository, HubSendMessage, HubTask, HubUpdateTask } from "./generated/types.js";
import { uuidv7 } from "./ids.js";
import { sleep } from "./sleep.js";

export interface CollaborationStatus {
  enabled: boolean;
  binding?: { hub_url: string; hub_id: string; project: string; repository: HubRepository } | null;
  current_member?: HubMember | null;
  published?: { session_id: string; agent_id?: string | null; alias: string; active: boolean; connected: boolean; last_error?: string | null } | null;
  error?: string | null;
  receipts?: Array<{ operation_id: string; state: "queued" | "applied" | "rejected"; detail?: string | null }>;
}
export interface CollaborationSnapshot {
  status: CollaborationStatus;
  members: HubMember[];
  agents: HubAgent[];
  messages: HubMessage[];
  tasks: HubTask[];
}
export interface CollaborationQueuedReceipt {
  status: "queued";
  operation_id: string;
  message: string;
}
export type CollaborationMutationResult<T> = T | CollaborationQueuedReceipt;
export function isCollaborationQueued(value: unknown): value is CollaborationQueuedReceipt {
  return typeof value === "object" && value !== null && "status" in value && value.status === "queued" && "operation_id" in value && typeof value.operation_id === "string" && "message" in value && typeof value.message === "string";
}
export interface CollaborationMutationOptions extends RequestOptions {
  /** Keep this key when explicitly retrying an unknown outcome. Defaults to a fresh UUID. */
  operationId?: string;
}
export interface CollaborationWatchOptions extends RequestOptions {
  /** Refresh interval, at least 250 ms (default 5000 ms). Authorization errors stop iteration. */
  intervalMs?: number;
}
export interface CollaborationCaller { readonly collaboration: Groups["collaboration"] }

/** A local Session/checkout context. Authors and credentials remain server-owned. */
export interface CollaborationHandle {
  status(options?: RequestOptions): Promise<CollaborationStatus>;
  snapshot(options?: RequestOptions): Promise<CollaborationSnapshot>;
  watch(options?: CollaborationWatchOptions): AsyncIterableIterator<CollaborationSnapshot>;
  send(message: Omit<HubSendMessage, "depth" | "hops">, options?: CollaborationMutationOptions): Promise<CollaborationMutationResult<HubMessage>>;
  createTask(task: HubCreateTask, options?: CollaborationMutationOptions): Promise<CollaborationMutationResult<HubTask>>;
  /** Carries the observed revision; a lost race rejects rather than stealing the task. */
  claim(task: Pick<HubTask, "id" | "revision">, options?: CollaborationMutationOptions): Promise<CollaborationMutationResult<HubTask>>;
  /** Carries both revision and claim generation; never silently retries stale work. */
  update(task: Pick<HubTask, "id" | "revision" | "claim_generation">, changes: Omit<HubUpdateTask, "expected_revision" | "claim_generation">, options?: CollaborationMutationOptions): Promise<CollaborationMutationResult<HubTask>>;
}

/** Local owner helpers. Connect a separate bearer-authenticated client for `client.hub`. */
export function collaborationSession(client: CollaborationCaller, sessionID: string, project?: string): CollaborationHandle {
  const context = { session_id: sessionID, ...(project !== undefined && { project }) };
  const status = async (options?: RequestOptions) => await client.collaboration.status(context, options) as CollaborationStatus;
  const snapshot = async (options?: RequestOptions): Promise<CollaborationSnapshot> => {
    const current = await status(options);
    if (!current.enabled || !current.binding) return { status: current, members: [], agents: [], messages: [], tasks: [] };
    const [people, messages, tasks] = await Promise.all([
      client.collaboration.participants(context, options),
      client.collaboration.messages(context, options),
      client.collaboration.tasks(context, options),
    ]);
    return { status: current, ...people as { members: HubMember[]; agents: HubAgent[] }, messages: messages as HubMessage[], tasks: tasks as HubTask[] };
  };
  const mutation = (payload: object, options: CollaborationMutationOptions = {}) => ({ ...context, ...payload, operation_id: options.operationId ?? uuidv7() });
  return {
    status,
    snapshot,
    watch: (options = {}) => watch(snapshot, options),
    send: async (message, options) => await client.collaboration.sendMessage(mutation(message, options), options) as CollaborationMutationResult<HubMessage>,
    createTask: async (task, options) => await client.collaboration.createTask(mutation(task, options), options) as CollaborationMutationResult<HubTask>,
    claim: async (task, options) => await client.collaboration.claimTask(task.id, mutation({ expected_revision: task.revision }, options), options) as CollaborationMutationResult<HubTask>,
    update: async (task, changes, options) => await client.collaboration.updateTask(task.id, mutation({ ...changes, expected_revision: task.revision, claim_generation: task.claim_generation }, options), options) as CollaborationMutationResult<HubTask>,
  };
}

async function* watch(snapshot: (options?: RequestOptions) => Promise<CollaborationSnapshot>, options: CollaborationWatchOptions): AsyncIterableIterator<CollaborationSnapshot> {
  const { intervalMs = 5000, ...request } = options;
  if (!Number.isFinite(intervalMs)) throw new RangeError("intervalMs must be finite");
  for (;;) {
    if (request.signal?.aborted) throw request.signal.reason ?? new Error("aborted");
    yield await snapshot(request);
    await sleep(Math.max(250, intervalMs), request.signal);
  }
}
