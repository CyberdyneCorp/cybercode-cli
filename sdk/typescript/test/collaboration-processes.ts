// Process isolation for actual-binary collaboration acceptance. Every machine has
// its own HOME/config/state/database and a clone with the same canonical Git remote.
import { execFile, spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";
import { Cyber, type CyberClient, readPassword, readRegistration } from "../src/index.js";

export function machineEnv(root: string, name: string): NodeJS.ProcessEnv {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (/^(CYBER_|XDG_|OPENAI_|ANTHROPIC_|AZURE_OPENAI_)/.test(key)) delete env[key];
  }
  return { ...env, HOME: join(root, name, "home"), CYBER_HOME: join(root, name, "installation"), CYBER_MODELS_PATH: join(root, "models.json"), DBUS_SESSION_BUS_ADDRESS: "" };
}

export function configureMachine(root: string, name: string, issuer: string, provider: Record<string, unknown>) {
  const env = machineEnv(root, name);
  mkdirSync(join(env.CYBER_HOME!, "config"), { recursive: true });
  mkdirSync(env.HOME!, { recursive: true });
  writeFileSync(join(env.CYBER_HOME!, "config", "cyber.json"), JSON.stringify({ ...provider, features: { collaboration: true }, account: { issuer, client_id: "cyber-cli", audience: "hub", allow_loopback_http: true } }));
  writeFileSync(join(root, "models.json"), "{}");
  return env;
}

export function execute(binary: string, args: string[], env: NodeJS.ProcessEnv, cwd: string): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(binary, args, { env, cwd, timeout: 30_000, maxBuffer: 1024 * 1024 }, (error, stdout, stderr) => {
      if (error) reject(new Error(`fixture command ${args[0]} failed: ${stderr.trim()}`, { cause: error }));
      else resolve(stdout);
    });
  });
}

export async function cloneMachines(root: string, names: string[]): Promise<Map<string, string>> {
  const source = join(root, "source");
  mkdirSync(source, { recursive: true });
  const env = machineEnv(root, "git");
  for (const key of Object.keys(env)) if (key.startsWith("GIT_")) delete env[key];
  Object.assign(env, { GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: join(root, "git-config"), GIT_TEMPLATE_DIR: join(root, "git-template"), GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.test", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.test" });
  mkdirSync(env.HOME!, { recursive: true });
  mkdirSync(env.GIT_TEMPLATE_DIR!, { recursive: true });
  writeFileSync(env.GIT_CONFIG_GLOBAL!, "");
  await execute("git", ["init", "-q", source], env, root);
  writeFileSync(join(source, "README.md"), "Fixture project\n");
  await execute("git", ["add", "README.md"], env, source);
  await execute("git", ["commit", "-qm", "Fixture project"], env, source);
  const paths = new Map<string, string>();
  for (const name of names) {
    const directory = join(root, name, "repo");
    mkdirSync(join(root, name), { recursive: true });
    await execute("git", ["clone", "-q", source, directory], env, root);
    await execute("git", ["remote", "set-url", "origin", "https://github.com/fixture/project.git"], env, directory);
    paths.set(name, directory);
  }
  return paths;
}

export async function startMachine(binary: string, env: NodeJS.ProcessEnv, directory: string) {
  await execute(binary, ["service", "start"], env, directory);
  const registration = await readRegistration(env);
  const password = await readPassword(env);
  if (!registration || !password) throw new Error("fixture service did not register its owner endpoint");
  const client = Cyber.connect({ baseUrl: registration.url, directory, auth: { type: "password", password }, retry: { retries: 0 } });
  return {
    client,
    url: registration.url,
    close: () => execute(binary, ["service", "stop"], env, directory).then(() => {}),
  };
}

export async function loginMachine(client: CyberClient, issuer: { authorize(url: string, subject: string): string }, subject: string) {
  const attempt = await client.account.login();
  if (!attempt.authorization_url) throw new Error("fixture account login has no authorization URL");
  await client.account.loginCode(attempt.attempt_id, { redirect_url: issuer.authorize(attempt.authorization_url, subject) });
  for (let n = 0; n < 100; n++) {
    const current = await client.account.loginAttempt(attempt.attempt_id);
    if (current.status === "complete") return;
    if (current.status !== "pending") throw new Error(`fixture account login failed: ${current.error}`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error("fixture account login did not complete");
}

export async function freePort(): Promise<number> {
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("fixture has no TCP address");
  const port = address.port;
  await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  return port;
}

export function startHub(binary: string, args: string[], env: NodeJS.ProcessEnv, directory: string) {
  const process = spawn(binary, ["collab", "serve", ...args], { env, cwd: directory, stdio: ["ignore", "ignore", "pipe"] });
  let error = "";
  process.stderr?.on("data", (chunk: Buffer) => { error = `${error}${chunk.toString()}`.slice(-8192); });
  return { process, diagnostic: () => error, close: () => stop(process) };
}
async function stop(process: ChildProcess) {
  if (process.exitCode !== null || process.signalCode !== null) return;
  await new Promise<void>((resolve) => {
    const timer = setTimeout(() => process.kill("SIGKILL"), 3000);
    process.once("exit", () => { clearTimeout(timer); resolve(); });
    process.kill("SIGTERM");
  });
}
