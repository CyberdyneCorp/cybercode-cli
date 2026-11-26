// Real-server snapshot recovery through the SDK. CI builds a debug CYBER_BIN, whose
// process-cut fixture point stops a non-Git restore after its first progress record;
// the registered service is then SIGKILLed and a fresh service resolves the journal.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Cyber, CyberApiError } from "../src/index.js";
import { startScriptedProvider } from "./scripted-provider.js";
import { guardRegisteredService } from "./service-guard.js";

const binary = process.env.CYBER_BIN;
const unix = process.platform === "darwin" || process.platform === "linux";
const skip = !binary ? "set CYBER_BIN to the cyber binary to run the end-to-end tests" : !unix ? "native recovery cuts are Unix-only" : false;
const FILES = ["cut_0000", "cut_0001", "cut_0002", "cut_0003"];

test("SDK resolves a killed partial restore and refuses stale and foreign requests without inference", { skip, timeout: 180_000 }, async (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-sdk-recovery-")));
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
  // Like the Rust fixtures: only the private homes and PATH reach the CLI and service.
  const minimal = (): NodeJS.ProcessEnv => ({
    HOME: process.env.HOME, CYBER_HOME: process.env.CYBER_HOME, CYBER_MODELS_PATH: process.env.CYBER_MODELS_PATH, PATH: process.env.PATH,
  });
  // Asynchronous (the scripted provider answers from this event loop) and with stdin
  // closed, as `exec` reads a piped stdin.
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
  // The process-cut service waits at its fixture point until it is killed, so a
  // failure before the kill below must not leave it, or the pending CLI, running.
  const killService = guardRegisteredService(process.env.CYBER_HOME!, binary!);
  let cut: ReturnType<typeof spawn> | undefined;
  t.after(async () => {
    await close?.();
    try { await cli(["service", "stop"]); } catch { /* already stopped */ }
    killService();
    cut?.kill("SIGKILL");
    await model.close();
    rmSync(root, { recursive: true, force: true });
  });
  const write = (contents: string) => FILES.forEach((name) => writeFileSync(join(repo, name), contents));
  const contents = () => FILES.map((name) => readFileSync(join(repo, name), "utf8"));
  write("before");
  const session = JSON.parse(await cli(["exec", "--mode", "accept-edits", "--format", "json", "before snapshot"])).session_id as string;
  write("after");
  await cli(["exec", "--mode", "accept-edits", "--format", "json", "--session", session, "after snapshot"]);
  const transcript = () => cli(["api", "GET", `/sessions/${session}/messages?order=asc&limit=200`]);
  const first = (JSON.parse(await transcript()).data as { id: string; kind: string }[]).find((m) => m.kind === "user")!.id;
  const preview = JSON.parse(await cli(["sessions", "rewind", session, "--to", first, "--target", "both", "--format", "json"]));
  await cli(["service", "stop"]);

  // Kill the service directly after the first exchange's progress record.
  const marker = join(root, "progress.marker");
  const pending = spawn(binary!, ["sessions", "rewind", session, "--to", first, "--target", "both", "--confirm", preview.confirmation, "--format", "json"], {
    cwd: repo, env: { ...minimal(), CYBER_TEST_PROCESS_CUT: "copy-restore-progress-recorded", CYBER_TEST_PROCESS_CUT_MARKER: marker }, stdio: "ignore",
  });
  cut = pending;
  const exited = new Promise<void>((resolve) => pending.once("exit", () => resolve()));
  const deadline = Date.now() + 120_000;
  while (!existsSync(marker)) {
    assert.ok(Date.now() < deadline && pending.exitCode === null, "the progress cut was never reached");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  const registration = JSON.parse(readFileSync(join(root, "cyber-home", "state", "server.json"), "utf8"));
  process.kill(registration.pid, "SIGKILL");
  await exited;
  assert.equal(contents().filter((value) => value === "before").length, 1);

  const server = await Cyber.start({ mode: "spawn", binary, directory: repo });
  close = () => server.close();
  const { client } = server;
  const before = await transcript();
  const inferred = inference;
  const journals = await client.session.snapshotRecovery(session);
  assert.equal(journals.length, 1);
  assert.equal(journals[0]!.filesystem_complete, false);
  const journal = journals[0]!.id;
  const review = await client.session.snapshotRecoveryReview(session, { journal_id: journal, action: "rollback" });
  assert.deepEqual(review.conflicts, []);
  const request = { journal_id: journal, action: "rollback" as const, confirmation: review.confirmation!, request_id: "sdk-recovery" };
  const conflict = (status: number, tag: string) => (error: unknown) =>
    error instanceof CyberApiError && error.status === status && error.tag === tag;
  await assert.rejects(() => client.session.snapshotRecoveryConfirm(session, { ...request, confirmation: "0".repeat(64) }), conflict(409, "ConflictError"));
  const foreign = (await client.session.create({ title: "foreign recovery" })).data.id;
  await assert.rejects(() => client.session.snapshotRecoveryConfirm(foreign, request), conflict(409, "ConflictError"));
  assert.equal(contents().filter((value) => value === "before").length, 1);

  const confirmed = await client.session.snapshotRecoveryConfirm(session, request);
  assert.equal(confirmed.state, "acknowledged");
  assert.equal(confirmed.journal_acknowledged, true);
  assert.deepEqual(contents(), FILES.map(() => "after"));
  // Lookup and a repeated confirmation never touch files again.
  writeFileSync(join(repo, FILES[0]!), "owner edit");
  const lookup = await client.session.snapshotRecoveryLookup(session, "sdk-recovery");
  assert.deepEqual(lookup.receipt, confirmed.receipt);
  assert.deepEqual((await client.session.snapshotRecoveryConfirm(session, request)).receipt, confirmed.receipt);
  await assert.rejects(() => client.session.snapshotRecoveryLookup(foreign, "sdk-recovery"), conflict(404, "RequestNotFoundError"));
  assert.equal(readFileSync(join(repo, FILES[0]!), "utf8"), "owner edit");
  assert.equal(await transcript(), before);
  assert.equal(inference, inferred);
});
