import assert from "node:assert/strict";
import { test } from "node:test";
import { SseParser, type SseMessage } from "../src/sse.js";
import { readSse } from "../src/sse.js";

const STREAM =
  ": heartbeat\r\n" +
  "id: s:1\r\nevent: session.created.1\r\ndata: {\"a\":1}\r\n\r\n" +
  "data: line one\ndata: line two\n\n" +
  "event: ignored-without-data\n\n" +
  "data:no-space\rid: s:3\r\r" +
  "data: {\"text\":\"héllo ✓\"}\n\n";

const EXPECTED: SseMessage[] = [
  { event: "session.created.1", data: '{"a":1}', id: "s:1" },
  { event: "message", data: "line one\nline two", id: "s:1" },
  { event: "message", data: "no-space", id: "s:3" },
  { event: "message", data: '{"text":"héllo ✓"}', id: "s:3" },
];

function parseChunks(chunks: string[]): SseMessage[] {
  const parser = new SseParser();
  return chunks.flatMap((chunk) => parser.push(chunk));
}

test("parses a stream delivered in one chunk", () => {
  assert.deepEqual(parseChunks([STREAM]), EXPECTED);
});

test("parses identically when split at every possible boundary", () => {
  for (let i = 0; i <= STREAM.length; i++) {
    assert.deepEqual(parseChunks([STREAM.slice(0, i), STREAM.slice(i)]), EXPECTED, `split at ${i}`);
  }
});

test("parses identically when fed one character at a time", () => {
  assert.deepEqual(parseChunks([...STREAM]), EXPECTED);
});

test("a CRLF split across chunks is one line ending", () => {
  assert.deepEqual(parseChunks(["data: a\r", "\ndata: b\r", "\n\r", "\n"]), [{ event: "message", data: "a\nb", id: undefined }]);
});

test("decodes UTF-8 sequences split across byte chunks", async () => {
  const bytes = new TextEncoder().encode(STREAM);
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const byte of bytes) controller.enqueue(new Uint8Array([byte]));
      controller.close();
    },
  });
  const out: SseMessage[] = [];
  for await (const message of readSse(body)) out.push(message);
  assert.deepEqual(out, EXPECTED);
});
