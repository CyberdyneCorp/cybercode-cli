// `plugins-marketplace` → TypeScript plugin kit: a plugin defined with the kit, driven
// through the JSON-RPC host protocol by the in-memory harness.
import assert from "node:assert/strict";
import { test } from "node:test";
import { PROTOCOL_VERSION, ProtocolFailure, createHarness, definePlugin, provider, tool, type SchemaLike } from "../src/index.js";

/** A Zod-style schema without a Zod dependency: parse plus `toJSONSchema()`. */
function named(): SchemaLike<{ name: string }> {
  return {
    safeParse(value: unknown) {
      const name = (value as { name?: unknown } | null)?.name;
      return typeof name === "string" && name.length > 0
        ? { success: true, data: { name: name.trim() } }
        : { success: false, error: new Error("name: expected a non-empty string") };
    },
    toJSONSchema: () => ({
      $schema: "https://json-schema.org/draft/2020-12/schema",
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
    }),
  };
}

test("define_plugin_tool_round_trip", async () => {
  let configured: unknown;
  const plugin = definePlugin({
    tools: {
      hello: tool({
        description: "Say hello",
        input: named(),
        annotations: { readOnlyHint: true },
        execute: ({ name }, context) => {
          context.log("info", `greeting ${name}`);
          return `Hello, ${name} (${context.callId}, site ${String(context.config.site)})`;
        },
      }),
      lookup: tool({
        description: "Structured answer from a JSON Schema input",
        input: { type: "object", properties: { id: { type: "integer" } } },
        execute: async (input: { id: number }, context) => ({
          id: input.id,
          token: await context.secret("api_token"),
          allowed: await context.askPermission("jira:write", "file an issue"),
        }),
      }),
    },
    hooks: {
      PreToolUse: (event) => (event.tool_name === "bash" ? { decision: "deny", reason: "no shell" } : undefined),
    },
    providers: {
      echo: provider({
        models: ["echo-1"],
        async *stream(request) {
          yield { type: "text", text: `echo ${request.model}` };
          yield { type: "usage", input_tokens: 1, output_tokens: 2 };
        },
      }),
    },
    setup: (config) => void (configured = config),
  });
  const harness = createHarness(plugin, {
    config: { site: "acme" },
    secrets: { api_token: "t0ken" },
    permission: (resource) => resource === "jira:write",
  });

  const initialized = await harness.initialize();
  assert.equal(initialized.protocolVersion, PROTOCOL_VERSION);
  assert.deepEqual(configured, { site: "acme" });
  assert.deepEqual(initialized.tools, [
    {
      name: "hello",
      description: "Say hello",
      inputSchema: { type: "object", properties: { name: { type: "string" } }, required: ["name"] },
      annotations: { readOnlyHint: true },
    },
    {
      name: "lookup",
      description: "Structured answer from a JSON Schema input",
      inputSchema: { type: "object", properties: { id: { type: "integer" } } },
    },
  ]);
  assert.deepEqual(initialized.hooks, [{ event: "PreToolUse" }]);
  assert.deepEqual(initialized.providers, [{ id: "echo", models: ["echo-1"] }]);

  const hello = await harness.callTool("hello", { name: " Ada " }, { callId: "call_1" });
  assert.deepEqual(hello, { content: "Hello, Ada (call_1, site acme)" });
  assert.deepEqual(harness.logs, [{ level: "info", message: "greeting Ada" }]);
  const invalid = await harness.callTool("hello", { name: "" });
  assert.equal(invalid.isError, true);
  assert.match(invalid.content, /invalid input: name: expected a non-empty string/);

  const lookup = await harness.callTool("lookup", { id: 7 });
  assert.deepEqual(lookup.structured, { id: 7, token: "t0ken", allowed: true });
  assert.equal(lookup.content, JSON.stringify({ id: 7, token: "t0ken", allowed: true }));
  assert.deepEqual(
    harness.requests.map((request) => request.method),
    ["secrets/get", "permission/ask"],
  );

  assert.deepEqual(await harness.hook("PreToolUse", { tool_name: "bash", tool_input: {}, session_id: "s" }), {
    decision: "deny",
    reason: "no shell",
  });
  assert.deepEqual(await harness.hook("PreToolUse", { tool_name: "read", tool_input: {}, session_id: "s" }), {});
  assert.deepEqual(await harness.stream("echo", { model: "echo-1", messages: [] }), [
    { type: "text", text: "echo echo-1" },
    { type: "usage", input_tokens: 1, output_tokens: 2 },
  ]);
  const unknown = await harness.callTool("missing", {}).catch((error: unknown) => error);
  assert.ok(unknown instanceof ProtocolFailure && /unknown tool missing/.test(unknown.message));
  await harness.shutdown();
});

test("a protocol version mismatch is refused with the kit's version", async () => {
  const harness = createHarness(definePlugin({}), { protocolVersion: "99" });
  const failure = await harness.initialize().catch((error: unknown) => error);
  assert.ok(failure instanceof ProtocolFailure);
  assert.equal(failure.code, -32602);
  assert.match(failure.message, /unsupported protocol version 99/);
  assert.deepEqual(failure.data, { protocolVersion: PROTOCOL_VERSION });
});

test("a cancelled call aborts the tool's signal", async () => {
  let aborted = false;
  const plugin = definePlugin({
    tools: {
      wait: tool({
        description: "Waits until cancelled",
        input: { type: "object" },
        execute: (_input, context) =>
          new Promise<string>((resolve) =>
            context.signal.addEventListener("abort", () => {
              aborted = true;
              resolve("stopped");
            }),
          ),
      }),
    },
  });
  const harness = createHarness(plugin);
  await harness.initialize();
  const controller = new AbortController();
  const call = harness.callTool("wait", {}, { signal: controller.signal });
  setTimeout(() => controller.abort(), 10);
  assert.deepEqual(await call, { content: "stopped" });
  assert.equal(aborted, true);
});

test("tool definitions are checked", () => {
  assert.throws(() => tool({ description: "x", input: { type: "string" }, execute: () => "" }), /must describe an object/);
  assert.throws(
    () => definePlugin({ tools: { "mcp__x": tool({ description: "x", input: { type: "object" }, execute: () => "" }) } }),
    /invalid tool name/,
  );
  const opaque = { safeParse: () => ({ success: true as const, data: {} }) };
  assert.throws(() => tool({ description: "x", input: opaque, execute: () => "" }), /pass jsonSchema/);
  const explicit = tool({ description: "x", input: opaque, jsonSchema: { type: "object" }, execute: () => "" });
  assert.deepEqual(explicit.inputSchema, { type: "object" });
});
