import type { Backoff } from "./connection.js";
import { isRetryable } from "./errors.js";
import type { EventEnvelope } from "./generated/types.js";
import { sleep } from "./sleep.js";

export interface EventStreamConfig {
  /** Open one connection; `after` is the last durable seq seen when resuming. */
  open(after: number | undefined, signal: AbortSignal): Promise<AsyncIterable<EventEnvelope>>;
  /** Resume from the last seen `durable.seq` and drop durable events at or below it. */
  resume: boolean;
  after?: number | undefined;
  backoff: Backoff;
  signal?: AbortSignal | undefined;
}

/**
 * Events from a stream (SSE or a JSON-RPC subscription) that reconnects with exponential backoff until the
 * caller stops iterating (`break`/`return`) or aborts `signal`. Client errors (4xx) end the
 * iteration with that error.
 */
export async function* eventStream(config: EventStreamConfig): AsyncGenerator<EventEnvelope, void, undefined> {
  const controller = new AbortController();
  const stop = () => controller.abort(config.signal?.reason);
  config.signal?.addEventListener("abort", stop, { once: true });
  if (config.signal?.aborted) stop();
  let after = config.after;
  let delay = config.backoff.initialMs;
  try {
    while (!controller.signal.aborted) {
      try {
        const events = await config.open(config.resume ? after : undefined, controller.signal);
        for await (const event of events) {
          delay = config.backoff.initialMs;
          const seq = event.durable?.seq;
          if (config.resume && seq !== undefined) {
            if (after !== undefined && seq <= after) continue;
            after = seq;
          }
          yield event;
        }
      } catch (err) {
        if (controller.signal.aborted) return;
        if (!isRetryable(err)) throw err;
      }
      await sleep(delay, controller.signal);
      delay = Math.min(delay * 2, config.backoff.maxMs);
    }
  } finally {
    config.signal?.removeEventListener("abort", stop);
    controller.abort();
  }
}
