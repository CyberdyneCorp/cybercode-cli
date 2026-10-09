import { Cyber, type ConnectOptions, type CyberClient } from "../src/index.js";

export const BASE = "http://cyber.test";

export interface Recorded {
  url: URL;
  method: string;
  headers: Headers;
  body: string | undefined;
  signal: AbortSignal | undefined;
}

export type Handler = (req: Recorded, index: number) => Response | Promise<Response>;

/** A client whose `fetch` is `handler`; every request is recorded in `calls`. */
export function mockClient(handler: Handler, options: ConnectOptions = {}): { client: CyberClient; calls: Recorded[] } {
  const calls: Recorded[] = [];
  const fetch = async (input: string | URL | Request, init: RequestInit = {}): Promise<Response> => {
    const req: Recorded = {
      url: new URL(String(input)),
      method: init.method ?? "GET",
      headers: new Headers(init.headers),
      body: typeof init.body === "string" ? init.body : undefined,
      signal: init.signal ?? undefined,
    };
    calls.push(req);
    return handler(req, calls.length - 1);
  };
  const client = Cyber.connect({
    baseUrl: BASE,
    auth: { type: "password", password: "secret" },
    fetch,
    retry: { delayMs: 0 },
    reconnect: { initialMs: 1, maxMs: 4 },
    ...options,
  });
  return { client, calls };
}

export function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

/** An SSE frame for an envelope. */
export function frame(event: { id: string; type: string; data?: unknown; location?: string; durable?: { aggregateID: string; seq: number; version: number } }): string {
  return `id: ${event.id}\nevent: ${event.type}\ndata: ${JSON.stringify({ data: {}, ...event })}\n\n`;
}

export function durable(session: string, seq: number, type: string, data: unknown = {}): string {
  return frame({ id: `${session}:${seq}`, type, data, durable: { aggregateID: session, seq, version: 1 } });
}

/** A response streaming `chunks`, then ending (or failing like a reset connection when `fail` is set). */
export function sse(chunks: string[], options: { fail?: boolean } = {}): Response {
  const encoder = new TextEncoder();
  const pending = [...chunks];
  // Pull-based so every chunk is read before the error: erroring a stream drops queued chunks.
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      const chunk = pending.shift();
      if (chunk !== undefined) controller.enqueue(encoder.encode(chunk));
      else if (options.fail) controller.error(new TypeError("terminated"));
      else controller.close();
    },
  });
  return new Response(body, { headers: { "content-type": "text/event-stream" } });
}

/** An SSE response the test writes to while the client reads; closes when the request aborts. */
export function liveSse(signal: AbortSignal | undefined): { response: Response; send(chunk: string): void } {
  const encoder = new TextEncoder();
  let controller!: ReadableStreamDefaultController<Uint8Array>;
  const body = new ReadableStream<Uint8Array>({
    start(c) {
      controller = c;
    },
  });
  signal?.addEventListener("abort", () => controller.error(signal.reason));
  return {
    response: new Response(body, { headers: { "content-type": "text/event-stream" } }),
    send: (chunk) => controller.enqueue(encoder.encode(chunk)),
  };
}
