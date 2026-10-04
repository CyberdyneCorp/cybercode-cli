/** One dispatched server-sent event. */
export interface SseMessage {
  event: string;
  data: string;
  id: string | undefined;
}

/**
 * An incremental `text/event-stream` parser (WHATWG HTML § 9.2.6). Feed it decoded text in
 * arbitrary chunks; it returns the events completed by each chunk.
 */
export class SseParser {
  private buffer = "";
  /** The previous chunk ended in `\r`, so a leading `\n` belongs to that line ending. */
  private pendingCr = false;
  private event = "";
  private data: string[] = [];
  private id: string | undefined;

  push(chunk: string): SseMessage[] {
    let text = chunk;
    if (this.pendingCr && text.startsWith("\n")) text = text.slice(1);
    this.pendingCr = text.endsWith("\r");
    this.buffer += text;
    const lines = this.buffer.split(/\r\n|\r|\n/);
    this.buffer = lines.pop() ?? "";
    const out: SseMessage[] = [];
    for (const line of lines) {
      const message = this.line(line);
      if (message) out.push(message);
    }
    return out;
  }

  private line(line: string): SseMessage | undefined {
    if (line === "") return this.dispatch();
    if (line.startsWith(":")) return undefined;
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "data") this.data.push(value);
    else if (field === "event") this.event = value;
    else if (field === "id" && !value.includes("\0")) this.id = value;
    return undefined;
  }

  private dispatch(): SseMessage | undefined {
    const message = this.data.length > 0 ? { event: this.event || "message", data: this.data.join("\n"), id: this.id } : undefined;
    this.event = "";
    this.data = [];
    return message;
  }
}

/** Parse a response body into events; ends when the body ends. */
export async function* readSse(body: ReadableStream<Uint8Array>): AsyncGenerator<SseMessage> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const parser = new SseParser();
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return;
      yield* parser.push(decoder.decode(value, { stream: true }));
    }
  } finally {
    reader.cancel().catch(() => {});
  }
}
