/**
 * The integration fixture (`editor-integration` → Extension packaging and CI, "Test
 * fixture isolation"): a temporary home, repository, password, IDE token and server
 * process, the real `cyber` binary and a loopback scripted OpenAI-compatible model. No real
 * credential is used. Everything is removed by `close()`.
 */
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createServer, type Server } from "node:http";
import { mkdtemp, mkdir, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

type Json = Record<string, unknown>;

/** What the test process inside VS Code needs, passed as one JSON environment variable. */
export interface FixtureInfo {
  root: string;
  home: string;
  repo: string;
  binary: string;
  modelUrl: string;
  serverUrl: string;
}

function lastUserText(messages: Json[]): string {
  const user = [...messages].reverse().find((message) => message.role === "user");
  const content = user?.content;
  if (typeof content === "string") return content;
  if (Array.isArray(content)) return content.map((part) => String((part as Json).text ?? "")).join("\n");
  return "";
}

function toolCalls(messages: Json[]): string[] {
  return messages.flatMap((message) => ((message.tool_calls as Json[] | undefined) ?? []).map((call) => String((call.function as Json).name)));
}

function call(name: string, input: Json, index: number): Json {
  return { tool_calls: [{ index: 0, id: `call_${index}`, type: "function", function: { name, arguments: JSON.stringify(input) } }] };
}

/** The scripted model: markers in the prompt choose a tool call; after it, a short reply. */
export function answer(body: Json): Json {
  const messages = (body.messages as Json[] | undefined) ?? [];
  const text = lastUserText(messages);
  const called = toolCalls(messages);
  if (text.includes("EDIT_APP")) {
    if (!called.includes("read")) return call("read", { path: "src/app.ts" }, messages.length);
    if (!called.includes("edit")) return call("edit", { path: "src/app.ts", old_string: "const value = 1;", new_string: "const value = 2;" }, messages.length);
  }
  const write = /WRITE_NEW:([a-z0-9_-]+)/.exec(text)?.[1];
  if (write && !called.includes("write")) return call("write", { path: `${write}.txt`, content: `created-${write}\n` }, messages.length);
  if (text.includes("PLAN_EXIT") && !called.includes("plan_exit")) return call("plan_exit", { summary: "Scripted plan" }, messages.length);
  if (messages.at(-1)?.role === "tool") return { content: "Completed the scripted action." };
  return { content: "Scripted reply from the fixture model." };
}

export class Fixture implements FixtureInfo {
  root = "";
  home = "";
  repo = "";
  modelUrl = "";
  serverUrl = "";
  readonly requests: Json[] = [];
  #model: Server | undefined;
  #server: ChildProcess | undefined;
  #output = "";

  constructor(readonly binary: string) {}

  get env(): NodeJS.ProcessEnv {
    return {
      PATH: `${dirname(this.binary)}:${process.env.PATH ?? ""}`,
      HOME: join(this.root, "home"),
      CYBER_HOME: this.home,
      CYBER_MODELS_PATH: join(this.root, "models.json"),
      CYBER_OFFLINE: "1",
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_CONFIG_GLOBAL: join(this.root, "gitconfig"),
    };
  }

  info(): FixtureInfo {
    return { root: this.root, home: this.home, repo: this.repo, binary: this.binary, modelUrl: this.modelUrl, serverUrl: this.serverUrl };
  }

  async create(): Promise<void> {
    // Short paths: macOS limits Unix socket paths to 104 bytes.
    // The real path: VS Code reports `/private/tmp/...` for macOS's `/tmp`.
    this.root = await realpath(await mkdtemp(join(process.platform === "darwin" ? "/tmp" : tmpdir(), "cyv-")));
    this.home = join(this.root, "h");
    this.repo = join(this.root, "repo");
    await Promise.all([mkdir(join(this.repo, "src"), { recursive: true }), mkdir(join(this.root, "home")), mkdir(join(this.home, "config"), { recursive: true })]);
    await this.startModel();
    await writeFile(join(this.root, "models.json"), "{}");
    await writeFile(join(this.root, "gitconfig"), "");
    await writeFile(join(this.home, "config/cyber.json"), JSON.stringify({
      providers: { test: { api: { type: "openai-compatible", url: `${this.modelUrl}/v1`, settings: { auth: "none" } }, models: { main: {} } } },
      model: "test/main",
      permissions: { edit: "ask", bash: "ask", read: { "*": "allow", "*.env": "deny" } },
    }));
    const lines = Array.from({ length: 12 }, (_, i) => (i === 3 ? "const value = 1;" : `// line ${i + 1}`));
    await writeFile(join(this.repo, "src/app.ts"), `${lines.join("\n")}\n`);
    await writeFile(join(this.repo, ".env"), "FIXTURE_SECRET=do-not-admit\n");
    for (const args of [["init", "--quiet", "-b", "main"], ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "--quiet", "-m", "fixture"]]) {
      const result = spawnSync("git", args, { cwd: this.repo, env: this.env });
      if (result.status !== 0) throw new Error("could not initialize the fixture repository");
    }
  }

  private async startModel(): Promise<void> {
    this.#model = createServer(async (request, response) => {
      const chunks: Buffer[] = [];
      for await (const part of request) chunks.push(Buffer.from(part));
      if (request.method === "GET") {
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify(this.requests));
        return;
      }
      const body = JSON.parse(Buffer.concat(chunks).toString()) as Json;
      this.requests.push(body);
      const delta = answer(body);
      const chunk = { choices: [{ index: 0, delta, finish_reason: "tool_calls" in delta ? "tool_calls" : "stop" }], usage: { prompt_tokens: 10, completion_tokens: 5 } };
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.end(`data: ${JSON.stringify(chunk)}\n\ndata: [DONE]\n\n`);
    });
    await new Promise<void>((resolve) => this.#model!.listen(0, "127.0.0.1", resolve));
    const address = this.#model.address() as { port: number };
    this.modelUrl = `http://127.0.0.1:${address.port}`;
  }

  async startServer(): Promise<void> {
    this.#server = spawn(this.binary, ["serve", "--register", "--hostname", "127.0.0.1", "--port", "0", "--sandbox", "full-access"], { cwd: this.repo, env: this.env, stdio: ["ignore", "pipe", "pipe"] });
    for (const stream of [this.#server.stdout, this.#server.stderr]) stream?.on("data", (chunk: Buffer) => (this.#output += chunk.toString()));
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline) {
      if (this.#server.exitCode !== null) throw new Error(`the fixture server exited: ${this.#output}`);
      const registration = await readFile(join(this.home, "state/server.json"), "utf8").then(JSON.parse).catch(() => undefined);
      if (registration?.url) {
        this.serverUrl = registration.url;
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    throw new Error("the fixture server did not register");
  }

  /** Run the real CLI with the fixture's environment. */
  cli(args: string[]): { status: number | null; stdout: string; stderr: string } {
    const result = spawnSync(this.binary, args, { cwd: this.repo, env: this.env, encoding: "utf8", timeout: 30_000 });
    return { status: result.status, stdout: result.stdout, stderr: result.stderr };
  }

  async close(): Promise<void> {
    const server = this.#server;
    if (server && server.exitCode === null) {
      server.kill("SIGTERM");
      await Promise.race([new Promise((resolve) => server.once("exit", resolve)), new Promise((resolve) => setTimeout(resolve, 5000))]);
      if (server.exitCode === null) server.kill("SIGKILL");
    }
    await new Promise<void>((resolve) => (this.#model ? this.#model.close(() => resolve()) : resolve()));
    if (this.root) await rm(this.root, { recursive: true, force: true });
  }
}
