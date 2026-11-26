// Real-server reviewed rewind through the SDK in a non-Git Location: code, conversation
// and both targets, a preview made stale by an editor write, the kept inactive branch,
// and no inference. The Session is produced by the real CLI, as a user would.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Cyber, CyberApiError } from "../src/index.js";
import { startScriptedProvider } from "./scripted-provider.js";
import { guardRegisteredService } from "./service-guard.js";

const binary = process.env.CYBER_BIN;
const unix = process.platform === "darwin" || process.platform === "linux";
const skip = !binary ? "set CYBER_BIN to the cyber binary to run the end-to-end tests" : !unix ? "non-Git snapshots are Unix-only" : false;

test("SDK rewinds code, conversation and both against a real server and refuses a stale preview", { skip, timeout: 180_000 }, async (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-sdk-rewind-")));
  let inference = 0;
  const model = await startScriptedProvider(() => { inference++; return undefined; });
  const config = join(root, "cyber-home", "config");
  mkdirSync(config, { recursive: true });
  writeFileSync(join(config, "cyber.jsonc"), JSON.stringify(model.config()));
  writeFileSync(join(root, "models.json"), "{}");
  const repo = join(root, "repo");
  mkdirSync(repo);
  for (const name of Object.keys(process.env)) {
    if (name.startsWith("CYBER_") || name.startsWith("XDG_")) delete process.env[name];
  }
  Object.assign(process.env, { HOME: join(root, "home"), CYBER_HOME: join(root, "cyber-home"), CYBER_MODELS_PATH: join(root, "models.json") });
  const minimal = (): NodeJS.ProcessEnv => ({
    HOME: process.env.HOME, CYBER_HOME: process.env.CYBER_HOME, CYBER_MODELS_PATH: process.env.CYBER_MODELS_PATH, PATH: process.env.PATH,
  });
  const cli = (args: string[]): Promise<string> => new Promise((resolve, reject) => {
    const child = spawn(binary!, args, { cwd: repo, env: minimal(), stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.once("error", reject);
    child.once("exit", (code) => (code === 0 ? resolve(stdout) : reject(new Error(`cyber ${args.join(" ")} exited ${code}: ${stderr}`))));
  });
  let close: (() => Promise<void>) | undefined;
  const killService = guardRegisteredService(process.env.CYBER_HOME!, binary!);
  t.after(async () => {
    await close?.();
    try { await cli(["service", "stop"]); } catch { /* already stopped */ }
    killService();
    await model.close();
    rmSync(root, { recursive: true, force: true });
  });

  const file = join(repo, "a.txt");
  writeFileSync(file, "before");
  const session = JSON.parse(await cli(["exec", "--mode", "accept-edits", "--format", "json", "first prompt"])).session_id as string;
  writeFileSync(file, "after");
  await cli(["exec", "--mode", "accept-edits", "--format", "json", "--session", session, "second prompt"]);
  await cli(["service", "stop"]);

  const server = await Cyber.start({ mode: "spawn", binary, directory: repo });
  close = () => server.close();
  const { client } = server;
  const users = async () => (await client.message.list(session, { order: "asc", limit: 200 })).data
    .filter((m: { kind: string }) => m.kind === "user").map((m: { id: string }) => m.id);
  const [first, second] = await users();
  assert.ok(first && second);
  const inferred = inference;
  const select = (message_id: string, target: "code" | "conversation" | "both") => ({ message_id, target, expected_directory: repo });
  const conflict = (error: unknown) => error instanceof CyberApiError && error.status === 409;

  // An editor write after review makes the code preview stale: nothing changes.
  const stale = await client.session.rewindPreview(session, select(first, "code"));
  assert.ok(stale.code, "a non-Git Location previews code");
  writeFileSync(file, "editor");
  await assert.rejects(() => client.session.rewindConfirm(session, { ...select(first, "code"), confirmation: stale.confirmation }), conflict);
  assert.equal(readFileSync(file, "utf8"), "editor");
  assert.deepEqual(await users(), [first, second]);

  // Code only: the file returns to the first boundary, the transcript stays.
  writeFileSync(file, "after");
  const code = await client.session.rewindPreview(session, select(first, "code"));
  await client.session.rewindConfirm(session, { ...select(first, "code"), confirmation: code.confirmation });
  assert.equal(readFileSync(file, "utf8"), "before");
  assert.deepEqual(await users(), [first, second]);

  // Conversation only: the later turn leaves the active path, files stay.
  const conversation = await client.session.rewindPreview(session, select(second, "conversation"));
  assert.equal(conversation.code ?? null, null);
  await client.session.rewindConfirm(session, { ...select(second, "conversation"), confirmation: conversation.confirmation });
  assert.equal(readFileSync(file, "utf8"), "before");
  assert.deepEqual(await users(), [first]);

  // Both: the whole conversation leaves the active path; the earlier branch is kept.
  const both = await client.session.rewindPreview(session, select(first, "both"));
  await client.session.rewindConfirm(session, { ...select(first, "both"), confirmation: both.confirmation });
  assert.deepEqual(await users(), []);
  const exported = await cli(["sessions", "export", session, "--all-branches"]);
  assert.ok(exported.includes(first) && exported.includes(second), "inactive branches remain exportable");
  // A confirmation is single-use.
  await assert.rejects(() => client.session.rewindConfirm(session, { ...select(first, "both"), confirmation: both.confirmation }));
  assert.equal(inference, inferred, "rewind never starts inference");
});
