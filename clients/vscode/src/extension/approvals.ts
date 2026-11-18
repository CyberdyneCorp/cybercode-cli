/**
 * Pending permission and question requests of the bound Session (`editor-integration` →
 * In-editor approvals). The host's own table decides what a reply may answer: only a
 * request currently pending for the bound Session (or one of its children), bound to its
 * exact Session and request ID, with one idempotency key per decision that every retry
 * reuses. A request answered elsewhere becomes stale and can no longer be answered.
 */
import { randomUUID } from "node:crypto";
import type { Card, Reply } from "../shared/protocol.js";

export interface PendingRequest {
  id: string;
  session_id: string;
  root_session_id?: string | null;
  kind: "permission" | "question";
  action?: string;
  resources?: string[];
  always_patterns?: string[];
  metadata?: unknown;
  questions?: { question: string; header: string; options: { label: string }[]; multi_select?: boolean; allow_custom?: boolean }[];
}

export type Decision = { reply: Reply; message?: string } | { answers?: string[][] };

interface Entry {
  request: PendingRequest;
  state: Card["state"];
  decision?: string;
  key?: string;
}

export interface Begun {
  key: string;
  request: PendingRequest;
}

function decisionKey(decision: Decision): string {
  return JSON.stringify(decision);
}

export class PendingTable {
  #session = "";
  readonly #entries = new Map<string, Entry>();

  /** Rebind to `session`: every request of another Session is dropped. */
  bind(session: string): void {
    if (session === this.#session) return;
    this.#session = session;
    this.#entries.clear();
  }

  get session(): string {
    return this.#session;
  }

  /** Whether `request` belongs to the bound Session or was routed to it from a child. */
  belongs(request: PendingRequest): boolean {
    return request.session_id === this.#session || request.root_session_id === this.#session;
  }

  /** The server's current list: new requests are added, missing ones become stale. */
  update(requests: PendingRequest[]): void {
    const listed = new Set<string>();
    for (const request of requests.filter((r) => this.belongs(r))) {
      listed.add(request.id);
      const entry = this.#entries.get(request.id);
      if (entry) entry.request = request;
      else this.#entries.set(request.id, { request, state: "pending" });
    }
    for (const [id, entry] of this.#entries) {
      if (!listed.has(id)) entry.state = "stale";
    }
  }

  /** A request answered elsewhere (`permission.replied.1`, `question.replied.1`). */
  answered(id: string): boolean {
    const entry = this.#entries.get(id);
    if (!entry) return false;
    entry.state = "stale";
    return true;
  }

  /** Drop stale entries once they were shown. */
  prune(): void {
    for (const [id, entry] of this.#entries) if (entry.state === "stale") this.#entries.delete(id);
  }

  get(id: string): PendingRequest | undefined {
    const entry = this.#entries.get(id);
    return entry && entry.state !== "stale" ? entry.request : undefined;
  }

  /**
   * Start a reply for `id` in `session`: `undefined` unless that request is pending for the
   * bound Session. A retry of an unconfirmed reply must be the same decision and reuses
   * its idempotency key.
   */
  begin(session: string, id: string, decision: Decision): Begun | undefined {
    const entry = this.#entries.get(id);
    if (!entry || entry.state === "stale" || entry.state === "sending" || entry.request.session_id !== session) return undefined;
    const wanted = decisionKey(decision);
    if (entry.state === "unconfirmed" && entry.decision !== wanted) return undefined;
    entry.key ??= randomUUID();
    entry.decision = wanted;
    entry.state = "sending";
    return { key: entry.key, request: entry.request };
  }

  /** The server acknowledged the reply: the request is gone. */
  acknowledge(id: string): void {
    this.#entries.delete(id);
  }

  /** The reply's outcome is unknown (the connection dropped): keep its key for the retry. */
  unconfirmed(id: string): void {
    const entry = this.#entries.get(id);
    if (entry) entry.state = "unconfirmed";
  }

  /** The server refused the reply outright: the decision may be changed. */
  refused(id: string): void {
    const entry = this.#entries.get(id);
    if (!entry) return;
    entry.state = "pending";
    entry.key = undefined;
    entry.decision = undefined;
  }

  /** Disable every card while the connection recovers. */
  suspend(): void {
    for (const entry of this.#entries.values()) if (entry.state === "pending") entry.state = "stale";
  }

  cards(): Card[] {
    return [...this.#entries.values()].map((entry) => card(entry, this.#session));
  }
}

function text(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === undefined || value === null) return "";
  return JSON.stringify(value, null, 2);
}

function card(entry: Entry, bound: string): Card {
  const request = entry.request;
  const metadata = (request.metadata ?? {}) as Record<string, unknown>;
  const detail = text(metadata.diff ?? metadata.command ?? metadata.description ?? "");
  return {
    kind: request.kind,
    sessionId: request.session_id,
    requestId: request.id,
    ...(request.session_id !== bound && { origin: request.session_id }),
    action: request.action ?? "question",
    resources: request.resources ?? [],
    detail,
    alwaysPatterns: request.always_patterns ?? [],
    ...(metadata.taint !== undefined && { taint: text(metadata.taint) }),
    questions: (request.questions ?? []).map((q) => ({
      header: q.header,
      question: q.question,
      options: q.options.map((option) => option.label),
      multiSelect: q.multi_select ?? false,
      allowCustom: q.allow_custom ?? false,
    })),
    state: entry.state,
    hasDiff: request.action === "edit" && typeof metadata.diff === "string",
  };
}
