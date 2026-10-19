# @cyber-code/plugin

Write Cyber Code plugins in TypeScript. A plugin that declares `main` in
`cyber-plugin.json` runs in its own sandboxed host process; the core speaks the JSON-RPC 2.0
host protocol to it over stdio (protocol version `1`). ESM, Node ≥ 20, no runtime
dependencies.

```ts
// index.mjs (compiled from index.ts), named by "main" in cyber-plugin.json
import { definePlugin, tool } from "@cyber-code/plugin";

export default definePlugin({
  tools: {
    hello: tool({
      description: "Greets someone",
      input: { type: "object", properties: { name: { type: "string" } }, required: ["name"] },
      annotations: { readOnlyHint: true },
      execute: ({ name }) => `Hello, ${name}`,
    }),
  },
});
```

```json
{ "name": "hello-kit", "version": "1.0.0", "main": "index.mjs" }
```

`cyber plugins install ./hello-kit --scope local` installs it; the model can then call
`hello`. When the core starts the module it sets `CYBER_PLUGIN_NAME`, and `definePlugin`
starts serving the protocol on stdio, so the default export is a complete `main`.

## API

- `tool({ description, input, output?, jsonSchema?, annotations?, execute })`. `input` is a
  JSON Schema object or a Zod-style schema (anything with `safeParse`); its JSON Schema comes
  from `toJSONSchema()` (Zod 4) or `jsonSchema`. A Zod-style `input` or `output` parses the
  value. A string result is the tool's text; any other value is returned as JSON and as the
  structured result. A thrown error is a failed call.
- `execute(input, context)`: `context` has `callId`, `sessionId`, `attempt`, `operationKey`,
  `signal` (aborted when the core cancels the call), `config` (the validated `userConfig`
  settings), `secret(key)` (needs the `secrets` capability), `askPermission(resource,
  message?)` and `log(level, message)`.
- `hooks: { PreToolUse, PostToolUse, UserPromptSubmit, SessionStart, Stop }`: typed handlers
  returning a decision. `provider({ models, stream })`: a provider adapter streamed over
  `provider/stream`. The core does not dispatch plugin hooks or providers yet; they are
  declared in `initialize` and can be tested with the harness.
- `createHarness(plugin, { config?, secrets?, permission? })` drives the protocol in memory:
  `initialize()`, `callTool(name, input)`, `hook(event, payload)`, `stream(provider,
  request)`, `configChanged(config)`, `shutdown()`, plus the `logs` and `requests` the
  plugin sent.

## Development

```sh
npm ci
npm run typecheck
npm test            # CYBER_BIN=<path to cyber> also runs the end-to-end test
```
