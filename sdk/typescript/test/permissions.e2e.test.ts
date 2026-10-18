// `client-sdk` → Location scoping against the real `cyber` binary: the rule dry run
// (`GET /permissions/test`) evaluates the client's Location, not the server's default
// directory. Set `CYBER_BIN` to the binary (CI builds `target/debug/cyber`); without it
// this test is skipped.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Cyber } from "../src/index.js";

const binary = process.env.CYBER_BIN;
const skip = binary ? false : "set CYBER_BIN to the cyber binary to run the end-to-end tests";

test("permission.test applies the client Location's project rules", { skip, timeout: 120_000 }, async (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-sdk-permissions-")));
  mkdirSync(join(root, "cyber-home", "config"), { recursive: true });
  writeFileSync(join(root, "models.json"), "{}");
  const [server, client] = [join(root, "server"), join(root, "client")];
  for (const repo of [server, client]) mkdirSync(join(repo, ".git"), { recursive: true });
  writeFileSync(join(client, "cyber.json"), JSON.stringify({ permissions: { edit: { "src/**": "deny" } } }));
  for (const name of Object.keys(process.env)) {
    if (name.startsWith("CYBER_") || name.startsWith("XDG_")) delete process.env[name];
  }
  Object.assign(process.env, { HOME: join(root, "home"), CYBER_HOME: join(root, "cyber-home"), CYBER_MODELS_PATH: join(root, "models.json") });
  const started = await Cyber.start({ mode: "embedded", binary, directory: server });
  t.after(async () => {
    await started.close();
    rmSync(root, { recursive: true, force: true });
  });
  const query = { tool: "write", resource: "src/a.rs", mode: "accept-edits" };
  type Report = { data: { results: { effect: string }[] } };
  const local = (await started.client.permission.test(query)) as unknown as Report;
  assert.equal(local.data.results[0]?.effect, "allow");
  const scoped = (await started.client.at(client).permission.test(query)) as unknown as Report;
  assert.equal(scoped.data.results[0]?.effect, "deny");
});
