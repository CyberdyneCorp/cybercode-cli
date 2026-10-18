import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { HookExecutionRecord } from "../src/generated/types.js";
import { Cyber, type HookInput } from "../src/index.js";
import { startScriptedProvider } from "./scripted-provider.js";
import { json, mockClient } from "./support.js";

const binary = process.env.CYBER_BIN;
const skip = binary ? false : "set CYBER_BIN to the cyber binary to run the end-to-end tests";

test("session hook receipts preserve pagination, unresolved status and IO omission", async () => {
  const record: HookExecutionRecord = {
    id: "hke_1", session_id: "ses_1", hook_id: "guard", digest: "sha256:reviewed",
    event: "PreToolUse", call_id: "call_1", tool_name: "write", scope: "global",
    directory: "/repo", agent: "build", mode: "default", started_ms: 1,
    log_io: false, once: false, status: "unknown", duration_ms: 2,
    outcome: "error", decision: {}, acknowledged: false, must_stop: true,
  };
  const { client, calls } = mockClient(() => json(200, { data: [record], cursor: { next: "1:ses_1:hke_1" } }), { directory: "/other" });
  const page = await client.session.hooks("ses/1", { limit: 1, cursor: "2:ses_1:hke_2" });
  assert.equal(page.data[0]?.status, "unknown");
  assert.equal(page.data[0]?.call_id, "call_1");
  assert.equal(page.data[0]?.io, undefined);
  assert.equal(page.cursor.next, "1:ses_1:hke_1");
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses%2F1/hook-executions");
  assert.equal(calls[0]?.url.searchParams.get("cursor"), "2:ses_1:hke_2");
  assert.equal(calls[0]?.url.searchParams.get("limit"), "1");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), null);
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
});

test("hook catalog retains Location, withheld definitions and current trust metadata", async () => {
  const { client, calls } = mockClient(() => json(200, {
    location: { directory: "/repo", project: { id: "global", directory: "/repo" } },
    data: { hooks: [], withheld_definitions: ["/repo/cyber.jsonc#/hooks"], checkout_trusted: false, withheld_hooks: [{ source:"/repo/cyber.jsonc", pointer:"/hooks", scope:"project", value:{PreToolUse:{command:"{env:UNREAD}"}} }] },
  }), { directory: "/repo" });
  const review = await client.hook.list();
  assert.equal(review.location.directory, "/repo");
  assert.equal(review.data.checkout_trusted, false);
  assert.equal(review.data.withheld_hooks[0]?.scope, "project");
  assert.deepEqual(review.data.withheld_hooks[0]?.value, {PreToolUse:{command:"{env:UNREAD}"}});
  assert.deepEqual(review.data.withheld_definitions, ["/repo/cyber.jsonc#/hooks"]);
  assert.equal(calls[0]?.method, "GET");
  assert.equal(calls[0]?.url.pathname, "/api/v1/hooks");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent("/repo"));
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
});

test("hook approval and revocation preserve exact digests and Location envelopes", async () => {
  const digest = "sha256:reviewed";
  const location = { directory: "/checkout A/nested", project: { id: "global", directory: "/checkout A" } };
  const { client, calls } = mockClient((_, index) => json(200, {
    location, data: index === 0 ? { digest } : { digest, revoked: true },
  }), { directory: location.directory });
  const approval = await client.hook.trust({ digest });
  const revocation = await client.hook.untrust({ digest });
  assert.equal(approval.data.digest, digest);
  assert.deepEqual(approval.location, location);
  assert.equal(revocation.data.revoked, true);
  assert.deepEqual(revocation.location, location);
  for (const [index, operation] of ["trust", "untrust"].entries()) {
    const call = calls[index]!;
    assert.equal(call.method, "POST");
    assert.equal(call.url.pathname, `/api/v1/hooks/${operation}`);
    assert.deepEqual(JSON.parse(call.body!), { digest });
    assert.equal(call.headers.get("x-cyber-directory"), encodeURIComponent(location.directory));
    assert.equal(call.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
  }
});

test("hook catalog last-run summaries retain unknown status without IO", async () => {
  const { client } = mockClient(() => json(200, {
    location: { directory: "/repo", project: { id: "global", directory: "/repo" } },
    data: { checkout_trusted:true, withheld_definitions:[], withheld_hooks:[], hooks:[{
      event:"PreToolUse", scope:"global", source:"/config/cyber.jsonc", pointer:"/hooks", matcher:null, paths:[],
      handler:{type:"command",command:"echo reviewed"},digest:"sha256:reviewed",trusted:true,sandbox_required:false,
      last_run:{id:"hke_latest",session_id:"ses_previous",directory:"/repo/nested",started_ms:42,duration_ms:1,status:"unknown",outcome:"error",acknowledged:false,must_stop:true},
    }] },
  }));
  const review = await client.hook.list();
  const last = review.data.hooks[0]?.last_run;
  assert.equal(last?.status,"unknown");
  assert.equal(last?.session_id,"ses_previous");
  assert.equal(last?.must_stop,true);
  assert.equal(last?.acknowledged,false);
  assert.equal("io" in (last ?? {}),false);
});

/** A `write` of `path` with id `id`, as the scripted model's streamed delta. */
function write(id: string, path: string): Record<string, unknown> {
  const args = JSON.stringify({ path, content: "edited\n" });
  return { tool_calls: [{ index: 0, id, type: "function", function: { name: "write", arguments: args } }] };
}

// `client-sdk` → Hook callbacks ("Approve edits programmatically") against the real binary: a
// `PreToolUse` handler that allows edits under `docs/` lets a `write` there run without a
// prompt (`edit` asks), and the execution is recorded with scope `client`.
test("pre_tool_use_client_handler_allows_docs_edits_and_records_client_scope", { skip, timeout: 120_000 }, async (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-sdk-hooks-")));
  const model = await startScriptedProvider((body) => {
    if (!body.includes("EDIT-DOCS")) return undefined;
    return body.includes("call_docs") ? { content: "edited the guide" } : write("call_docs", "docs/guide.md");
  });
  const config = join(root, "cyber-home", "config");
  mkdirSync(config, { recursive: true });
  writeFileSync(join(config, "cyber.jsonc"), JSON.stringify({ ...model.config(), permissions: { edit: "ask" } }));
  writeFileSync(join(root, "models.json"), "{}");
  const repo = join(root, "repo");
  mkdirSync(join(repo, ".git"), { recursive: true });
  for (const name of Object.keys(process.env)) {
    if (name.startsWith("CYBER_") || name.startsWith("XDG_")) delete process.env[name];
  }
  Object.assign(process.env, { HOME: join(root, "home"), CYBER_HOME: join(root, "cyber-home"), CYBER_MODELS_PATH: join(root, "models.json") });
  const server = await Cyber.start({ mode: "embedded", binary, directory: repo });
  t.after(async () => {
    await server.close();
    await model.close();
    rmSync(root, { recursive: true, force: true });
  });
  const { client } = server;
  const seen: HookInput[] = [];
  const off = await client.hooks.on(
    "PreToolUse",
    (input) => {
      seen.push(input);
      const path = (input.tool_input as { path?: string } | undefined)?.path ?? "";
      return path.startsWith("docs/") ? { decision: "allow", reason: "docs edits are pre-approved" } : undefined;
    },
    { matcher: "write" },
  );
  const asked: unknown[] = [];
  const stop = client.permissions.onRequest((request) => {
    asked.push(request);
    return "reject";
  });
  const created = await client.session.create({ model: "test/main" });
  const id = created.data.id;
  await client.sessions.prompt(id, "EDIT-DOCS", { wait: true });
  assert.equal(readFileSync(join(repo, "docs", "guide.md"), "utf8"), "edited\n");
  assert.deepEqual(asked, []);
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.event, "PreToolUse");
  assert.equal(seen[0]?.session_id, id);
  assert.equal(seen[0]?.tool_name, "write");
  const receipts = await client.session.hooks(id);
  const recorded = receipts.data.filter((record) => record.scope === "client");
  assert.equal(recorded.length, 1);
  assert.equal(recorded[0]?.event, "PreToolUse");
  assert.equal(recorded[0]?.status, "completed");
  assert.equal(recorded[0]?.decision?.decision, "allow");
  assert.ok(recorded[0]?.hook_id.startsWith("client:hkr_"));
  stop();
  await off();
  assert.ok(!existsSync(join(repo, "src")));
});
