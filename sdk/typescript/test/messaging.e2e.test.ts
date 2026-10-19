// `cross-session-messaging` → Messaging API through the SDK against the real `cyber`
// binary: `client.messaging.send()` as a client and as a Session, then `history()` reads
// both sides across pages. Set `CYBER_BIN` to the binary (CI builds `target/debug/cyber`);
// without it this test is skipped.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Cyber, type MessageRecord } from "../src/index.js";
import { startScriptedProvider } from "./scripted-provider.js";

const binary = process.env.CYBER_BIN;
const skip = binary ? false : "set CYBER_BIN to the cyber binary to run the end-to-end tests";

async function collect<T>(items: AsyncIterable<T>): Promise<T[]> {
  const all: T[] = [];
  for await (const item of items) all.push(item);
  return all;
}

const sides = (records: MessageRecord[]) => records.map((r) => [r.message_id, r.direction, r.outcome, r.origin_kind]);

test("messaging.send and messaging.history page both sides of a conversation", { skip, timeout: 120_000 }, async (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-sdk-messaging-")));
  const model = await startScriptedProvider();
  const config = join(root, "cyber-home", "config");
  mkdirSync(config, { recursive: true });
  const messaging = { inbound: { hold: ["frontend"] } };
  writeFileSync(join(config, "cyber.jsonc"), JSON.stringify({ ...model.config(), messaging }));
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
  const frontend = (await client.session.create({ title: "frontend" })).data.id;
  const backend = (await client.session.create({ title: "backend" })).data.id;

  const sent = await client.messaging.send({ id: "msgx_1_sdk", to: "backend", text: "deploy at noon" });
  assert.equal(sent.outcome, "delivered");
  assert.equal(sent.to.session_id, backend);
  const held = await client.messaging.send({ id: "msgx_2_held", from: frontend, to: "backend", text: "build is red" });
  assert.equal(held.outcome, "held");
  const replay = await client.messaging.send({ id: "msgx_1_sdk", to: "backend", text: "deploy at noon" });
  assert.equal(replay.replayed, true);

  // One record per page, read lazily; no record twice.
  const received = await collect(client.messaging.history(backend, { limit: 1, order: "asc" }));
  assert.deepEqual(sides(received), [
    ["msgx_1_sdk", "in", "delivered", "client"],
    ["msgx_2_held", "in", "held", "session"],
  ]);
  assert.equal(received[0]?.from, "client:api");
  assert.equal(received[0]?.text, "deploy at noon");
  assert.ok(!Number.isNaN(Date.parse(received[1]!.created_at)));
  const outgoing = await collect(client.messaging.history(frontend, { direction: "out" }));
  assert.deepEqual(sides(outgoing), [["msgx_2_held", "out", "held", "session"]]);
  assert.equal(outgoing[0]?.to, "backend");
  assert.deepEqual(await collect(client.messaging.history(frontend, { direction: "in" })), []);
});
