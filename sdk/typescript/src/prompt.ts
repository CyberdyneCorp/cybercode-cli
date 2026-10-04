import { CyberClientError } from "./errors.js";
import type { Groups, RequestOptions } from "./generated/operations.js";
import type { Content, Delivery, EventEnvelope, Receipt, Usage } from "./generated/types.js";

export type PromptInput = string | Content[];

export interface PromptOptions extends RequestOptions {
  /** `steer` (default), `queue` or `hold`. */
  delivery?: Delivery;
  /** Switch the Session's agent, model or mode before prompting (persists, like the CLI flags). */
  agent?: string;
  model?: string;
  mode?: string;
  /** Resolve when the Session next becomes idle, with the final text, usage and stop reason. */
  wait?: boolean;
}

/** Why the Drain stopped: the last step's finish reason, or `error` when a step failed. */
export type StopReason = "stop" | "tool_calls" | "length" | "content_filter" | "error" | (string & {});

export interface PromptResult {
  receipt: Receipt;
  /** The last assistant text of the Drain (`session.text.ended.1`), or `""`. */
  text: string;
  /** Token usage summed over the Drain's steps. */
  usage: Usage;
  stopReason: StopReason;
  /** The failure message when `stopReason` is `error`. */
  error?: string;
}

type Client = Pick<Groups, "session" | "event">;

export function toParts(input: PromptInput): Content[] {
  return typeof input === "string" ? [{ type: "text", text: input }] : input;
}

/** The Session an event belongs to: the durable aggregate, or `data.session_id` for live events. */
export function sessionOf(event: EventEnvelope): string | undefined {
  if (event.durable) return event.durable.aggregateID;
  const data = event.data as { session_id?: unknown } | null;
  return typeof data?.session_id === "string" ? data.session_id : undefined;
}

export async function switchSession(client: Client, id: string, options: PromptOptions): Promise<void> {
  const call = { signal: options.signal } satisfies RequestOptions;
  if (options.agent !== undefined) await client.session.agent(id, { agent: options.agent }, call);
  if (options.model !== undefined) await client.session.model(id, { model: options.model }, call);
  if (options.mode !== undefined) await client.session.mode(id, { mode: options.mode }, call);
}

/** Post a prompt with the instance stream already subscribed, then follow the Drain to idle. */
export async function promptAndWait(client: Client, id: string, parts: Content[], options: PromptOptions): Promise<PromptResult> {
  if (options.delivery === "hold") throw new TypeError("a held prompt does not run; `wait` needs `steer` or `queue` delivery");
  const controller = new AbortController();
  const signal = options.signal ? AbortSignal.any([options.signal, controller.signal]) : controller.signal;
  const events = client.event.subscribe({ scope: "all" }, { signal });
  try {
    // The first event (`server.connected`) proves the server is subscribed for us.
    if ((await events.next()).done) throw new CyberClientError("Transport", "event stream closed before it connected");
    const receipt = await client.session.prompt(id, { parts, delivery: options.delivery ?? "steer" }, options);
    return await followDrain(events, id, receipt);
  } finally {
    controller.abort();
    await events.return?.(undefined);
  }
}

async function followDrain(events: AsyncIterator<EventEnvelope>, id: string, receipt: Receipt): Promise<PromptResult> {
  const drain = new DrainResult(receipt);
  for (;;) {
    const next = await events.next();
    if (next.done) throw new CyberClientError("Transport", "event stream ended before the session became idle");
    if (sessionOf(next.value) === id && drain.add(next.value)) return drain.result();
  }
}

/** Accumulates the events of the Drain that runs the admitted prompt. */
class DrainResult {
  private text = "";
  private usage: Usage = { input: 0, output: 0, reasoning: 0, cache_read: 0, cache_write: 0 };
  private stopReason: StopReason = "stop";
  private error: string | undefined;
  /** A durable event after the admission was seen, so the next idle ends this Drain. */
  private started = false;

  constructor(private readonly receipt: Receipt) {}

  /** Record an event of the Session; true once the Session is idle after the prompt ran. */
  add(event: EventEnvelope): boolean {
    const seq = event.durable?.seq;
    if (seq !== undefined) {
      if (seq <= this.receipt.admitted_seq) return false;
      this.started = true;
    }
    const data = (event.data ?? {}) as Record<string, unknown>;
    switch (event.type) {
      case "session.text.ended.1":
        this.text = String(data.text ?? "");
        break;
      case "session.step.ended.1":
        this.addUsage(data.usage as Partial<Usage> | undefined);
        this.stopReason = finishReason(data.finish);
        break;
      case "session.step.failed.1":
      case "session.error":
        this.stopReason = "error";
        this.error = String(data.message ?? "");
        break;
      case "session.idle":
        return this.started;
    }
    return false;
  }

  result(): PromptResult {
    const result: PromptResult = { receipt: this.receipt, text: this.text, usage: this.usage, stopReason: this.stopReason };
    if (this.error !== undefined) result.error = this.error;
    return result;
  }

  private addUsage(usage: Partial<Usage> | undefined): void {
    for (const key of Object.keys(this.usage) as (keyof Usage)[]) this.usage[key] += Number(usage?.[key] ?? 0);
  }
}

/** `FinishReason` serializes as a string, or `{ "other": "..." }`. */
function finishReason(finish: unknown): StopReason {
  if (typeof finish === "string") return finish;
  const other = (finish as { other?: unknown } | null)?.other;
  return typeof other === "string" ? other : "stop";
}
