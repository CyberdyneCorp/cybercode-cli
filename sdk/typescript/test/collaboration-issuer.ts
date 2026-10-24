// Local signed multi-principal Auth fixture. Each access token has independent live
// authorization so revocation tests do not accidentally share Alice's identity.
import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, randomUUID, sign } from "node:crypto";
import { once } from "node:events";
import { createServer, type IncomingMessage } from "node:http";

interface User {
  name: string;
  active: boolean;
  orgs: Array<{ id: string; short_name: string }>;
  auth_time: number;
}
interface Grant { subject: string; nonce: string; challenge: string; redirect: string }

export async function startCollaborationIssuer() {
  const keys = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const jwk = { ...keys.publicKey.export({ format: "jwk" }), kid: "collaboration-fixture", alg: "RS256", use: "sig" };
  const users = new Map<string, User>();
  const grants = new Map<string, Grant>();
  const tokens = new Map<string, string>();
  let issuer = "";
  let checks = 0;
  const now = () => Math.floor(Date.now() / 1000);
  const jwt = (subject: string, audience: string, extra: Record<string, unknown>) => {
    const user = users.get(subject);
    assert.ok(user, `unknown fixture subject ${subject}`);
    const head = Buffer.from(JSON.stringify({ alg: "RS256", kid: jwk.kid, typ: "JWT" })).toString("base64url");
    const payload = Buffer.from(JSON.stringify({ iss: issuer, sub: subject, aud: audience, iat: now(), exp: now() + 3600, auth_time: user.auth_time, amr: ["pwd", "otp"], name: user.name, ...extra })).toString("base64url");
    const body = `${head}.${payload}`;
    return `${body}.${sign("RSA-SHA256", Buffer.from(body), keys.privateKey).toString("base64url")}`;
  };
  const access = (subject: string) => {
    const token = jwt(subject, "hub", { scope: "openid cyber:collaborate", orgs: users.get(subject)!.orgs, jti: randomUUID() });
    tokens.set(token, subject);
    return token;
  };
  const body = async (req: IncomingMessage) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(Buffer.from(chunk));
    return new URLSearchParams(Buffer.concat(chunks).toString());
  };
  const server = createServer(async (req, res) => {
    const reply = (value: unknown, status = 200) => { res.statusCode = status; res.setHeader("content-type", "application/json"); res.end(JSON.stringify(value)); };
    try {
      switch (req.url) {
        case "/.well-known/openid-configuration":
          return reply({ issuer, authorization_endpoint: `${issuer}/authorize`, token_endpoint: `${issuer}/token`, jwks_uri: `${issuer}/keys`, introspection_endpoint: `${issuer}/introspect`, scopes_supported: ["openid", "profile", "email", "offline_access", "cyber:collaborate"], code_challenge_methods_supported: ["S256"], grant_types_supported: ["authorization_code", "refresh_token", "client_credentials"], id_token_signing_alg_values_supported: ["RS256"] });
        case "/keys": return reply({ keys: [jwk] });
        case "/introspect": {
          checks++;
          if (req.headers.authorization !== "Bearer fixture-backend-only") return reply({ error: "invalid_client" }, 401);
          const form = await body(req);
          const subject = tokens.get(form.get("token") ?? "");
          const user = subject && users.get(subject);
          if (!user) return reply({ active: false });
          return reply({ active: user.active, token_type: "user", sub: subject, aud: "hub", exp: now() + 3600, scope: "openid cyber:collaborate", orgs: user.orgs, auth_time: user.auth_time, amr: ["pwd", "otp"], name: user.name });
        }
        case "/token": {
          const form = await body(req);
          if (form.get("grant_type") === "client_credentials") {
            if (req.headers.authorization !== `Basic ${Buffer.from("fixture-hub:fixture-secret").toString("base64")}`) return reply({ error: "invalid_client" }, 401);
            return reply({ access_token: "fixture-backend-only", token_type: "Bearer", expires_in: 600 });
          }
          assert.equal(req.headers.authorization, undefined);
          assert.equal(form.get("client_secret"), null);
          assert.equal(form.get("client_id"), "cyber-cli");
          const code = form.get("code") ?? "";
          const grant = grants.get(code);
          grants.delete(code);
          if (!grant) return reply({ error: "invalid_grant" }, 400);
          assert.equal(form.get("redirect_uri"), grant.redirect);
          assert.equal(createHash("sha256").update(form.get("code_verifier") ?? "").digest("base64url"), grant.challenge);
          return reply({ access_token: access(grant.subject), id_token: jwt(grant.subject, "cyber-cli", { nonce: grant.nonce, email: `${grant.subject}@example.test` }), token_type: "Bearer" });
        }
        default: reply({ error: "not_found" }, 404);
      }
    } catch (error) { reply({ error: String(error) }, 400); }
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  issuer = `http://127.0.0.1:${address.port}`;
  return {
    issuer,
    add(subject: string, organizations: string[] = ["org-team"]) {
      users.set(subject, { name: subject, active: true, orgs: organizations.map((id) => ({ id, short_name: id })), auth_time: now() });
      return access(subject);
    },
    token: access,
    setActive(subject: string, active: boolean) { users.get(subject)!.active = active; },
    setOrganizations(subject: string, organizations: string[]) { users.get(subject)!.orgs = organizations.map((id) => ({ id, short_name: id })); },
    authorize(authorizationUrl: string, subject: string) {
      assert.ok(users.has(subject));
      const authorization = new URL(authorizationUrl);
      assert.equal(authorization.origin, issuer);
      const code = randomUUID();
      const redirect = authorization.searchParams.get("redirect_uri")!;
      grants.set(code, { subject, nonce: authorization.searchParams.get("nonce")!, challenge: authorization.searchParams.get("code_challenge")!, redirect });
      const callback = new URL(redirect);
      callback.searchParams.set("state", authorization.searchParams.get("state")!);
      callback.searchParams.set("code", code);
      return callback.toString();
    },
    get checks() { return checks; },
    close: () => new Promise<void>((resolve, reject) => { server.close((error) => error ? reject(error) : resolve()); server.closeAllConnections(); }),
  };
}
