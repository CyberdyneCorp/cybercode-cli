// Real owner-client byte path. CI builds CYBER_BIN; no external provider is used.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Cyber, CyberApiError } from "../src/index.js";
import { startScriptedProvider } from "./scripted-provider.js";

const binary = process.env.CYBER_BIN;
const skip = binary ? false : "set CYBER_BIN to the cyber binary to run the end-to-end tests";

test("binary helper reads every byte, preserves scope and releases real owner documents without inference", { skip, timeout: 120_000 }, async (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-sdk-binary-")));
  let inference = 0;
  const model = await startScriptedProvider(() => { inference++; return undefined; });
  const config = join(root, "cyber-home", "config");
  mkdirSync(config, { recursive: true });
  writeFileSync(join(config, "cyber.jsonc"), JSON.stringify(model.config()));
  writeFileSync(join(root, "models.json"), "{}");
  const repo = join(root, "repo");
  mkdirSync(join(repo, ".git"), { recursive: true });
  const source = join(repo, "bytes.bin");
  const raw = Buffer.from(Array.from({ length: 256 }, (_, n) => n));
  writeFileSync(source, raw);
  for (const name of Object.keys(process.env)) {
    if (name.startsWith("CYBER_") || name.startsWith("XDG_")) delete process.env[name];
  }
  Object.assign(process.env, { HOME: join(root, "home"), CYBER_HOME: join(root, "cyber-home"), CYBER_MODELS_PATH: join(root, "models.json") });
  let close: (() => Promise<void>) | undefined;
  t.after(async () => {
    await close?.();
    await model.close();
    rmSync(root, { recursive: true, force: true });
  });
  const server = await Cyber.start({ mode: "embedded", binary, directory: repo });
  close = () => server.close();
  const { client } = server;
  const session = (await client.session.create({ title: "binary owner" })).data.id;
  const foreign = (await client.session.create({ title: "another owner" })).data.id;
  const document = await client.binary.open(session, "bytes.bin", { expectedRoot: repo });
  assert.equal(document.metadata.length, "256");
  assert.deepEqual(await document.bytes(0n, 256), new Uint8Array(raw));
  assert.deepEqual((await document.search("feff")).offsets, ["254"]);
  assert.deepEqual(await document.bytes(256n, 1), new Uint8Array());
  await assert.rejects(() => client.fs.binaryBytes(document.id, {
    session_id: foreign, revision: document.metadata.revision, offset: "0", limit: 1,
  }), (error: unknown) => error instanceof CyberApiError && error.status === 404);
  assert.deepEqual(readFileSync(source), raw);

  // A replacement cannot supply bytes through an old descriptor/revision.
  renameSync(source, join(repo, "old.bin"));
  writeFileSync(source, Buffer.from([255]));
  await assert.rejects(() => document.bytes(0n, 1), (error: unknown) => error instanceof CyberApiError && error.status === 400);
  await document.close();
  // More than the per-Session admission limit over time proves close releases slots.
  for (let n = 0; n < 6; n++) {
    const reopened = await client.binary.open(session, "bytes.bin");
    assert.deepEqual(await reopened.bytes(0n, 1), new Uint8Array([255]));
    await reopened.close();
  }
  assert.equal(inference, 0);
});
