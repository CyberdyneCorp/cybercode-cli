// Account SDK operations through a real server and a local RS256 OIDC issuer.
// Linux exercises the owner-only production fallback; native vault layouts have
// injected Rust integration tests, so this suite never writes real OS Keychains.
import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { once } from "node:events";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Cyber } from "../src/index.js";
import { startScriptedProvider } from "./scripted-provider.js";

const binary = process.env.CYBER_BIN;
const skip = !binary ? "set CYBER_BIN to run end-to-end tests" : process.platform !== "linux" ? "uses the Linux file fallback; no native Keychain writes" : false;

test("account SDK drives verified login, display and logout through the real server", { skip, timeout: 120_000 }, async (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-sdk-account-")));
  const keys = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const publicJwk = { ...keys.publicKey.export({ format: "jwk" }), kid: "fixture", alg: "RS256", use: "sig" };
  let issuer = "";
  let nonce = "";
  let challenge = "";
  const jwt = (aud: string, extra: Record<string, unknown>) => {
    const issued = Math.floor(Date.now() / 1000);
    const head = Buffer.from(JSON.stringify({ alg: "RS256", kid: "fixture", typ: "JWT" })).toString("base64url");
    const claims = Buffer.from(JSON.stringify({ iss: issuer, sub: "alice", aud, exp: issued + 3600, iat: issued, auth_time: issued, amr: ["pwd", "otp"], ...extra })).toString("base64url");
    const body = `${head}.${claims}`;
    return `${body}.${sign("RSA-SHA256", Buffer.from(body), keys.privateKey).toString("base64url")}`;
  };
  const auth = createServer(async (req, res) => {
    const reply = (value: unknown) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify(value)); };
    if (req.url === "/.well-known/openid-configuration") {
      reply({ issuer, authorization_endpoint: `${issuer}/authorize`, token_endpoint: `${issuer}/token`, jwks_uri: `${issuer}/keys`, scopes_supported: ["openid", "profile", "email", "offline_access", "cyber:collaborate"], code_challenge_methods_supported: ["S256"], grant_types_supported: ["authorization_code", "refresh_token"], id_token_signing_alg_values_supported: ["RS256"] });
    } else if (req.url === "/keys") {
      reply({ keys: [publicJwk] });
    } else if (req.url === "/token") {
      const parts: Buffer[] = [];
      for await (const chunk of req) parts.push(Buffer.from(chunk));
      const form = new URLSearchParams(Buffer.concat(parts).toString());
      assert.equal(form.get("client_id"), "cyber-cli");
      assert.equal(form.get("code"), "fixture-code");
      assert.equal(createHash("sha256").update(form.get("code_verifier")!).digest("base64url"), challenge);
      assert.equal(form.get("client_secret"), null);
      reply({ access_token: jwt("hub", { scope: "openid cyber:collaborate", orgs: [{ id: "org-uuid", short_name: "team" }] }), id_token: jwt("cyber-cli", { nonce, email: "alice@example.test" }), refresh_token: "fixture-refresh-never-returned", token_type: "Bearer" });
    } else { res.statusCode = 404; res.end(); }
  });
  auth.listen(0, "127.0.0.1");
  await once(auth, "listening");
  const address = auth.address();
  assert.ok(address && typeof address !== "string");
  issuer = `http://127.0.0.1:${address.port}`;
  const model = await startScriptedProvider();
  const config = join(root, "installation", "config");
  mkdirSync(config, { recursive: true });
  writeFileSync(join(config, "cyber.json"), JSON.stringify({ ...model.config(), account: { issuer, client_id: "cyber-cli", audience: "hub", allow_loopback_http: true } }));
  writeFileSync(join(root, "models.json"), "{}");
  const repo = join(root, "repo");
  mkdirSync(join(repo, ".git"), { recursive: true });
  for (const name of Object.keys(process.env)) if (name.startsWith("CYBER_") || name.startsWith("XDG_")) delete process.env[name];
  Object.assign(process.env, { HOME: join(root, "home"), CYBER_HOME: join(root, "installation"), CYBER_MODELS_PATH: join(root, "models.json") });
  const server = await Cyber.start({ mode: "spawn", binary, directory: repo });
  t.after(async () => {
    await server.close();
    await model.close();
    await new Promise<void>((resolve) => auth.close(() => resolve()));
    rmSync(root, { recursive: true, force: true });
  });
  const { client } = server;
  assert.equal((await client.account.get()).logged_in, false);
  const local = (await client.session.create({ title: "kept across logout" })).data.id;
  const started = (await client.account.login());
  const authorization = new URL(started.authorization_url!);
  nonce = authorization.searchParams.get("nonce")!;
  challenge = authorization.searchParams.get("code_challenge")!;
  const redirect = new URL(authorization.searchParams.get("redirect_uri")!);
  redirect.searchParams.set("state", authorization.searchParams.get("state")!);
  redirect.searchParams.set("code", "fixture-code");
  await client.account.loginCode(started.attempt_id, { redirect_url: redirect.toString() });
  let state = (await client.account.loginAttempt(started.attempt_id));
  for (let n = 0; state.status === "pending" && n < 100; n++) {
    await new Promise((resolve) => setTimeout(resolve, 20));
    state = (await client.account.loginAttempt(started.attempt_id));
  }
  assert.equal(state.status, "complete", state.error ?? "login did not complete");
  const current = (await client.account.get());
  assert.equal(current.sub, "alice");
  assert.equal(current.email, "alice@example.test");
  assert.equal(current.storage, "file");
  assert.ok(!JSON.stringify(current).includes("fixture-refresh-never-returned"));
  assert.ok(!JSON.stringify(current).includes("access_token"));
  await client.account.logout({ all: "true" });
  assert.equal((await client.account.get()).logged_in, false);
  assert.equal((await client.session.get(local)).id, local);
});
