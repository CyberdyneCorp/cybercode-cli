/**
 * Messages between the chat webview and the extension host (`editor-integration` →
 * Extension webview isolation). Both sides validate every message: unknown types, extra
 * fields, payloads over 1 MiB and malformed IDs are dropped. No message ever carries a
 * credential; the webview holds none.
 */

export const MAX_MESSAGE_BYTES = 1024 * 1024;
export const SESSION_ID = /^ses_[A-Za-z0-9]+$/;
export const REQUEST_ID = /^(per|que)_[A-Za-z0-9]+$/;
export const MODES = ["default", "accept-edits", "plan", "auto"] as const;
export type Mode = (typeof MODES)[number];
export type Reply = "once" | "always" | "reject";
export type HostAction = "createToken" | "startServer" | "manageTrust" | "installCli" | "saveDocument" | "retry" | "reviewChanges" | "openTerminal";

/** From the webview to the extension host. */
export type WebviewMessage =
  | { type: "ready" }
  | { type: "prompt"; text: string; includeContext: boolean }
  | { type: "interrupt" }
  | { type: "newSession" }
  | { type: "selectSession"; sessionId: string }
  | { type: "selectFolder"; index: number }
  | { type: "setMode"; mode: Mode }
  | { type: "setModel"; model: string }
  | { type: "reply"; sessionId: string; requestId: string; reply: Reply; message?: string }
  | { type: "answer"; sessionId: string; requestId: string; answers?: string[][] }
  | { type: "review"; requestId: string }
  | { type: "loadOlder" }
  | { type: "openLink"; url: string }
  | { type: "action"; action: HostAction };

export interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "system" | "tool";
  text: string;
}

export interface Card {
  kind: "permission" | "question";
  sessionId: string;
  requestId: string;
  /** The Session that asked, when it is a child of the bound one. */
  origin?: string;
  action: string;
  resources: string[];
  detail: string;
  alwaysPatterns: string[];
  taint?: string;
  questions: { header: string; question: string; options: string[]; multiSelect: boolean; allowCustom: boolean }[];
  /** `pending`, `sending`, `unconfirmed` (sent, acknowledgement lost) or `stale`. */
  state: "pending" | "sending" | "unconfirmed" | "stale";
  hasDiff: boolean;
}

export interface ViewState {
  phase: "starting" | "restricted" | "noCli" | "noServer" | "noToken" | "incompatible" | "refused" | "connecting" | "ready";
  notice: string;
  actions: HostAction[];
  folders: string[];
  folder: number;
  sessions: { id: string; title: string }[];
  sessionId: string;
  mode: string;
  pendingMode: string;
  model: string;
  models: string[];
  busy: boolean;
  messages: ChatMessage[];
  hasOlder: boolean;
  live: string;
  cards: Card[];
  chip: { label: string; note: string } | null;
}

/** From the extension host to the webview. */
export type HostMessage =
  | { type: "state"; state: ViewState }
  | { type: "insert"; text: string }
  | { type: "confirmLink"; url: string };

type Fields = Record<string, (value: unknown) => boolean>;

const str = (value: unknown): boolean => typeof value === "string";
const bool = (value: unknown): boolean => typeof value === "boolean";
const sessionId = (value: unknown): boolean => typeof value === "string" && SESSION_ID.test(value);
const requestId = (value: unknown): boolean => typeof value === "string" && REQUEST_ID.test(value);
const oneOf = (values: readonly string[]) => (value: unknown) => typeof value === "string" && values.includes(value);
const answers = (value: unknown): boolean =>
  Array.isArray(value) && value.every((row) => Array.isArray(row) && row.every(str));
const optional = (check: (value: unknown) => boolean) => (value: unknown) => value === undefined || check(value);

const ACTIONS: readonly HostAction[] = ["createToken", "startServer", "manageTrust", "installCli", "saveDocument", "retry", "reviewChanges", "openTerminal"];

const WEBVIEW: Record<WebviewMessage["type"], Fields> = {
  ready: {},
  prompt: { text: str, includeContext: bool },
  interrupt: {},
  newSession: {},
  selectSession: { sessionId },
  selectFolder: { index: (value) => Number.isInteger(value) && (value as number) >= 0 },
  setMode: { mode: oneOf(MODES) },
  setModel: { model: (value) => typeof value === "string" && /^[\w.\-/#:]{1,200}$/.test(value) },
  reply: { sessionId, requestId, reply: oneOf(["once", "always", "reject"]), message: optional(str) },
  answer: { sessionId, requestId, answers: optional(answers) },
  review: { requestId },
  loadOlder: {},
  openLink: { url: str },
  action: { action: oneOf(ACTIONS) },
};

const HOST: Record<HostMessage["type"], Fields> = {
  state: { state: (value) => typeof value === "object" && value !== null },
  insert: { text: str },
  confirmLink: { url: str },
};

/** `value` when it is a well-formed message of `table`, else `undefined`. */
function check<T>(table: Record<string, Fields>, value: unknown): T | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
  const record = value as Record<string, unknown>;
  const fields = typeof record.type === "string" ? table[record.type] : undefined;
  if (!fields || !Object.hasOwn(table, record.type as string)) return undefined;
  const known = Object.keys(record).every((key) => key === "type" || Object.hasOwn(fields, key));
  const valid = Object.entries(fields).every(([key, ok]) => ok(record[key]));
  if (!known || !valid) return undefined;
  return withinLimit(value) ? (value as T) : undefined;
}

/** Whether `value` serializes to at most [`MAX_MESSAGE_BYTES`]. */
export function withinLimit(value: unknown): boolean {
  try {
    return new TextEncoder().encode(JSON.stringify(value)).length <= MAX_MESSAGE_BYTES;
  } catch {
    return false;
  }
}

export function webviewMessage(value: unknown): WebviewMessage | undefined {
  return check<WebviewMessage>(WEBVIEW, value);
}

export function hostMessage(value: unknown): HostMessage | undefined {
  return check<HostMessage>(HOST, value);
}
