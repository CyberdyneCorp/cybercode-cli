// `plugins-marketplace` → TypeScript plugin kit, against the real `cyber` binary: a plugin
// whose `main` exports `definePlugin({ tools: { hello: tool({...}) } })` is installed with
// `cyber plugins install ./my-plugin --scope local`, and the model calls `hello` in
// `cyber exec`. The model is a local scripted one; nothing reaches the internet. Set
// `CYBER_BIN` to the binary (CI builds `target/debug/cyber`); without it this test is skipped.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { test } from "node:test";

const binary = process.env.CYBER_BIN;
const skip = binary ? false : "set CYBER_BIN to the cyber binary to run the end-to-end tests";

/** Calls `hello` first, then answers with what the tool returned. */
async function scriptedModel(): Promise<{ url: string; bodies: string[]; close: () => Promise<void> }> {
  const bodies: string[] = [];
  const server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      const body = Buffer.concat(chunks).toString("utf8");
      bodies.push(body);
      const parsed = JSON.parse(body) as { messages: { role: string; content: unknown }[] };
      const last = parsed.messages[parsed.messages.length - 1];
      const delta =
        last?.role === "tool"
          ? { content: `Answer: ${String(last.content)}` }
          : body.includes("Write a short title")
            ? { content: "Plugin" }
            : {
                tool_calls: [
                  { index: 0, id: "call_hello", type: "function", function: { name: "hello", arguments: '{"name":"Ada"}' } },
                ],
              };
      const chunk = {
        choices: [{ index: 0, delta, finish_reason: "tool_calls" in delta ? "tool_calls" : "stop" }],
        usage: { prompt_tokens: 100, completion_tokens: 10 },
      };
      res.writeHead(200, { "content-type": "text/event-stream", connection: "close" });
      res.end(`data: ${JSON.stringify(chunk)}\n\ndata: [DONE]\n\n`);
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  return {
    url,
    bodies,
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

test("a TypeScript plugin tool is callable by the model", { skip, timeout: 120_000 }, async (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-plugin-e2e-")));
  const model = await scriptedModel();
  t.after(async () => {
    await model.close();
    rmSync(root, { recursive: true, force: true });
  });
  const repo = join(root, "repo");
  const plugin = join(repo, "my-plugin");
  mkdirSync(join(repo, ".git"), { recursive: true });
  mkdirSync(plugin, { recursive: true });
  mkdirSync(join(root, "cyber-home", "config"), { recursive: true });
  writeFileSync(join(root, "models.json"), "{}");
  writeFileSync(
    join(root, "cyber-home", "config", "cyber.jsonc"),
    JSON.stringify({
      providers: { test: { api: { type: "openai-compatible", url: `${model.url}/v1`, settings: { auth: "none" } }, models: { main: {} } } },
      model: "test/main",
    }),
  );
  // The kit as this test run compiled it.
  const kit = pathToFileURL(join(import.meta.dirname, "..", "src", "index.js")).href;
  writeFileSync(
    join(plugin, "cyber-plugin.json"),
    JSON.stringify({ name: "hello-kit", version: "1.0.0", main: "index.mjs" }),
  );
  writeFileSync(
    join(plugin, "index.mjs"),
    `import { definePlugin, tool } from ${JSON.stringify(kit)};
export default definePlugin({
  tools: {
    hello: tool({
      description: "Greets someone",
      input: { type: "object", properties: { name: { type: "string" } }, required: ["name"] },
      annotations: { readOnlyHint: true },
      execute: ({ name }) => \`Hello, \${name}, from TypeScript\`,
    }),
  },
});
`,
  );
  const env: Record<string, string> = {
    PATH: process.env.PATH ?? "",
    HOME: join(root, "home"),
    CYBER_HOME: join(root, "cyber-home"),
    CYBER_DB: "test.db",
    CYBER_MODELS_PATH: join(root, "models.json"),
    CYBER_MANAGED_POLICY_DIR: join(root, "no-policy"),
  };
  if (process.env.CYBER_SANDBOX_HELPER) env.CYBER_SANDBOX_HELPER = process.env.CYBER_SANDBOX_HELPER;
  // Asynchronous: the scripted model answers from this process's event loop.
  const cyber = (...args: string[]) =>
    new Promise<{ status: number | null; stdout: string; stderr: string }>((resolve) => {
      const child = spawn(binary!, args, { cwd: repo, env, stdio: ["ignore", "pipe", "pipe"], timeout: 90_000 });
      let stdout = "";
      let stderr = "";
      child.stdout.on("data", (chunk: Buffer) => (stdout += chunk.toString("utf8")));
      child.stderr.on("data", (chunk: Buffer) => (stderr += chunk.toString("utf8")));
      child.on("close", (status) => resolve({ status, stdout, stderr }));
    });

  const installed = await cyber("plugins", "install", "./my-plugin", "--scope", "local");
  assert.equal(installed.status, 0, installed.stderr);
  assert.match(installed.stdout, /Installed hello-kit@1\.0\.0 \(local scope/);
  assert.match(installed.stderr, /host: node index\.mjs/);

  const run = await cyber("exec", "--embedded", "--model", "test/main", "Say hello to Ada");
  assert.equal(run.status, 0, `${run.stdout}\n${run.stderr}`);
  assert.match(run.stdout, /Answer: Hello, Ada, from TypeScript/);
  const first = model.bodies.find((body) => !body.includes("Write a short title"));
  assert.ok(first?.includes('"name":"hello"'), "hello is advertised to the model");
});
