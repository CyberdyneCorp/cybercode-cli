// A local OpenAI-compatible model for the end-to-end tests: every streamed chat request is
// answered with one content chunk. A request whose body mentions `GATE-<label>` waits until
// `open(label)`, then answers `<label> done`; any other request answers `agent done`. It
// never reaches the internet.
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";

export interface ScriptedProvider {
  /** `http://127.0.0.1:<port>`. */
  readonly url: string;
  /** Labels of gated requests still waiting, in arrival order. */
  held(): string[];
  /** Let every request gated on `label` answer, now and later. */
  open(label: string): void;
  /** The global configuration naming this model as provider `test`, model `main`. */
  config(): Record<string, unknown>;
  close(): Promise<void>;
}

export async function startScriptedProvider(): Promise<ScriptedProvider> {
  const waiting: { label: string; release: () => void }[] = [];
  const opened = new Set<string>();
  const server = createServer((req, res) => void answer(req, res));

  async function answer(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    const label = /GATE-([a-z0-9]+)/.exec(Buffer.concat(chunks).toString("utf8"))?.[1];
    if (label !== undefined && !opened.has(label)) await new Promise<void>((release) => waiting.push({ label, release }));
    const content = label === undefined ? "agent done" : `${label} done`;
    const chunk = {
      choices: [{ index: 0, delta: { content }, finish_reason: "stop" }],
      usage: { prompt_tokens: 100, completion_tokens: 10 },
    };
    res.writeHead(200, { "content-type": "text/event-stream", connection: "close" });
    res.end(`data: ${JSON.stringify(chunk)}\n\ndata: [DONE]\n\n`);
  }

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  return {
    url,
    held: () => waiting.map((w) => w.label),
    open(label) {
      opened.add(label);
      for (const w of waiting.filter((w) => w.label === label)) w.release();
      waiting.splice(0, waiting.length, ...waiting.filter((w) => w.label !== label));
    },
    config: () => ({
      providers: { test: { api: { type: "openai-compatible", url: `${url}/v1`, settings: { auth: "none" } }, models: { main: {} } } },
      model: "test/main",
    }),
    close: async () => {
      for (const w of waiting.splice(0)) w.release();
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}
