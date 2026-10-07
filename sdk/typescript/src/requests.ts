import type { Groups } from "./generated/operations.js";
import type { PendingRequest, ReplyKind } from "./generated/types.js";

export type PermissionRequest = Extract<PendingRequest, { kind: "permission" }>;
export type QuestionRequest = Extract<PendingRequest, { kind: "question" }>;

export type PermissionDecision = ReplyKind | { reply: ReplyKind; message?: string };
export type PermissionHandler = (request: PermissionRequest) => PermissionDecision | Promise<PermissionDecision>;
/** Selected labels (or custom text) per question; `undefined` dismisses the question. */
export type QuestionHandler = (request: QuestionRequest) => string[][] | undefined | Promise<string[][] | undefined>;

export interface OnRequestOptions {
  /** Called when the handler or the reply fails (default: `console.error`). */
  onError?: (err: unknown) => void;
}

type Client = Pick<Groups, "event" | "location" | "permission" | "question" | "session">;

interface Watch<R> {
  /** The durable event that announces a request. */
  type: string;
  /** Requests already pending, fetched on every (re)connect. */
  pending(): Promise<PendingRequest[]>;
  handle(request: R): Promise<void>;
}

/** Answer permission requests of the client's Location until the returned function is called. */
export function onPermissionRequest(client: Client, handler: PermissionHandler, options: OnRequestOptions = {}): () => void {
  return watch<PermissionRequest>(client, options, {
    type: "permission.asked.1",
    pending: () => client.permission.list(),
    handle: async (request) => {
      const decision = await handler(request);
      const body = typeof decision === "string" ? { reply: decision } : decision;
      await client.permission.reply(request.session_id, request.id, body);
    },
  });
}

/** Answer questions of the client's Location until the returned function is called. */
export function onQuestionRequest(client: Client, handler: QuestionHandler, options: OnRequestOptions = {}): () => void {
  return watch<QuestionRequest>(client, options, {
    type: "question.asked.1",
    pending: () => client.question.list(),
    handle: async (request) => {
      const answers = await handler(request);
      await client.question.reply(request.session_id, request.id, answers === undefined ? {} : { answers });
    },
  });
}

/**
 * Follow the Location's events: on every (re)connect (`server.connected`) handle requests that
 * are already pending, then each asked event. Requests are handled once each, concurrently; a
 * failed one is retried on the next reconnect.
 */
function watch<R extends PendingRequest>(client: Client, options: OnRequestOptions, spec: Watch<R>): () => void {
  const controller = new AbortController();
  const report = options.onError ?? ((err: unknown) => console.error(`cyber: ${spec.type} handler failed`, err));
  const handled = new Set<string>();
  const inLocation = locationFilter(client);
  const dispatch = (request: R) => {
    if (handled.has(request.id)) return;
    handled.add(request.id);
    spec.handle(request).catch((err: unknown) => {
      handled.delete(request.id);
      report(err);
    });
  };
  const catchUp = async () => {
    const pending = (await spec.pending()).filter((r) => !handled.has(r.id));
    for (const request of await inLocation(pending)) dispatch(request as R);
  };
  const run = async () => {
    for await (const event of client.event.subscribe(undefined, { signal: controller.signal })) {
      if (event.type === "server.connected") catchUp().catch(report);
      else if (event.type === spec.type) dispatch(event.data as R);
    }
  };
  run().catch((err: unknown) => {
    if (!controller.signal.aborted) report(err);
  });
  return () => controller.abort();
}

/** Keep requests owned in this Location or routed here by an ancestor (lists span all Locations). */
function locationFilter(client: Client): (requests: PendingRequest[]) => Promise<PendingRequest[]> {
  const directories = new Map<string, string>();
  const directoryOf = async (sessionID: string) => {
    if (!directories.has(sessionID)) directories.set(sessionID, (await client.session.get(sessionID)).directory);
    return directories.get(sessionID);
  };
  return async (requests) => {
    if (requests.length === 0) return [];
    const { directory } = await client.location.get();
    const keep = await Promise.all(requests.map(async (r) =>
      r.routed_to?.some((route) => route.directory === directory) || (await directoryOf(r.session_id)) === directory,
    ));
    return requests.filter((_, i) => keep[i]);
  };
}
