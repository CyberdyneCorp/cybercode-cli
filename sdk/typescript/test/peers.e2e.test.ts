// OpenAPI group `peer` through the SDK against the real `cyber` binary
// (`cross-session-messaging` → Direct peer transport): `client.peers.invite()` on the main
// API returns a pairing code, and a second client on the peer listener (plain HTTP, which
// is allowed only on loopback) calls the generated `peer` group with the grant's bearer
// token. Set `CYBER_BIN` to the binary (CI builds `target/debug/cyber`); without it this
// test is skipped.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Cyber, CyberApiError, isPeerScopeError } from "../src/index.js";
import { startScriptedProvider } from "./scripted-provider.js";

const binary = process.env.CYBER_BIN;
const skip = binary ? false : "set CYBER_BIN to the cyber binary to run the end-to-end tests";

test("the peer group reaches the peer listener with a grant's token", { skip, timeout: 120_000 }, async (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-sdk-peers-")));
  const model = await startScriptedProvider();
  const config = join(root, "cyber-home", "config");
  mkdirSync(config, { recursive: true });
  const peers = { listen: { address: "127.0.0.1:0", tls: false }, machine: "desk" };
  writeFileSync(join(config, "cyber.jsonc"), JSON.stringify({ ...model.config(), peers }));
  writeFileSync(join(root, "models.json"), "{}");
  const repo = join(root, "repo");
  mkdirSync(join(repo, ".git"), { recursive: true });
  for (const name of Object.keys(process.env)) {
    if (name.startsWith("CYBER_") || name.startsWith("XDG_")) delete process.env[name];
  }
  Object.assign(process.env, { HOME: join(root, "home"), CYBER_HOME: join(root, "cyber-home"), CYBER_MODELS_PATH: join(root, "models.json") });
  const server = await Cyber.start({ mode: "spawn", binary, directory: repo });
  t.after(async () => {
    await server.close();
    await model.close();
    rmSync(root, { recursive: true, force: true });
  });
  const { client } = server;
  const migration = (await client.session.create({ title: "migration" })).data.id;

  const invitation = await client.peers.invite({ name: "laptop" });
  assert.equal(invitation.listener.tls, false);
  assert.ok(invitation.listener.url.startsWith("http://127.0.0.1:"), invitation.listener.url);
  const token = new URL(invitation.code.replace(/^cyberpeer:/, "http:")).searchParams.get("t");
  assert.ok(token?.startsWith("cyp_"), "the pairing code carries the token");
  assert.deepEqual(await client.peers.remotes(), [], "no remotes in user config");

  const peer = Cyber.connect({ baseUrl: invitation.listener.url, auth: { type: "account", token: token! } });
  const hello = await peer.peer.hello();
  assert.deepEqual([hello.machine, hello.grant, hello.scopes], ["desk", "laptop", ["message"]]);
  assert.equal(hello.fingerprint, invitation.listener.fingerprint);
  const listed = await peer.peer.sessions();
  assert.deepEqual(
    listed.sessions.map((s) => [s.session_id, s.location.directory]),
    [[migration, "repo"]],
    "the grant sees the Session by its workspace name only",
  );
  const status = await peer.peer.status(migration);
  assert.equal(status.state, "idle");

  // The grant has no attach scope.
  await assert.rejects(peer.peer.attachOpen(migration), (err: unknown) => isPeerScopeError(err) && err.status === 403);
  // The main API's password is not a peer credential, and the peer token is not a main one.
  const password = Cyber.connect({ baseUrl: invitation.listener.url, auth: { type: "password", password: "x" }, retry: { retries: 0 } });
  await assert.rejects(password.peer.hello(), (err: unknown) => err instanceof CyberApiError && err.status === 401);
  const wrong = Cyber.connect({ baseUrl: server.registration!.url, auth: { type: "account", token: token! }, retry: { retries: 0 } });
  await assert.rejects(wrong.session.list(), (err: unknown) => err instanceof CyberApiError && err.status === 401);
});
