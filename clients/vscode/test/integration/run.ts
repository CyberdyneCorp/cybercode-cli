/**
 * Launches VS Code with a temporary profile against the fixture (`@vscode/test-electron`),
 * twice: a trusted workspace for the chat suite, and Restricted Mode. Afterwards it checks
 * that the IDE token appears nowhere in the profile (settings, extension storage, logs)
 * and removes everything.
 */
import { readdir, readFile, mkdir, mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { downloadAndUnzipVSCode } from "@vscode/test-electron";
import { Fixture } from "./fixture";

const extension = resolve(__dirname, "../..");
const repository = resolve(extension, "../..");

async function files(dir: string): Promise<string[]> {
  const entries = await readdir(dir, { withFileTypes: true }).catch(() => []);
  const nested = await Promise.all(entries.map((entry) => (entry.isDirectory() ? files(join(dir, entry.name)) : [join(dir, entry.name)])));
  return nested.flat();
}

/** Every file of `dir` that contains `secret`. */
async function holding(dir: string, secret: string): Promise<string[]> {
  const found: string[] = [];
  for (const file of await files(dir)) {
    if ((await stat(file)).size > 64 * 1024 * 1024) continue;
    if ((await readFile(file)).includes(secret)) found.push(file);
  }
  return found;
}

/** Print the report's new lines while a suite runs, so a stalled run shows where it is. */
function follow(report: string): () => Promise<void> {
  let printed = 0;
  const flush = async () => {
    const text = await readFile(report, "utf8").catch(() => "");
    if (text.length > printed) process.stdout.write(text.slice(printed));
    printed = text.length;
  };
  const timer = setInterval(() => void flush(), 2000);
  return async () => {
    clearInterval(timer);
    await flush();
  };
}

async function launch(fixture: Fixture, suite: "chat" | "restricted", profile: string): Promise<void> {
  const report = join(profile, `${suite}.report`);
  const stop = follow(report);
  try {
    await runSuite(fixture, suite, profile, report);
  } finally {
    await stop();
  }
}

async function runSuite(fixture: Fixture, suite: "chat" | "restricted", root: string, report: string): Promise<void> {
  const profile = join(root, suite);
  const trust = suite === "restricted" ? [] : ["--disable-workspace-trust"];
  // A fresh profile per suite: no AI features, update checks or welcome pages, and, for
  // Restricted Mode, nothing trusted and no startup prompt.
  const settings: Record<string, unknown> = {
    "chat.disableAIFeatures": true,
    "workbench.startupEditor": "none",
    "update.mode": "none",
    "extensions.autoUpdate": false,
    "extensions.autoCheckUpdates": false,
    "telemetry.telemetryLevel": "off",
    "git.enabled": false,
  };
  if (suite === "restricted") {
    Object.assign(settings, { "security.workspace.trust.enabled": true, "security.workspace.trust.startupPrompt": "never", "security.workspace.trust.untrustedFiles": "open" });
  }
  await mkdir(join(profile, "user/User"), { recursive: true });
  await writeFile(join(profile, "user/User/settings.json"), JSON.stringify(settings));
  const env = { ...fixture.env, CYBER_VSCODE_TEST: "1", CYBER_TEST_SUITE: suite, CYBER_TEST_FIXTURE: JSON.stringify(fixture.info()), CYBER_TEST_REPORT: report };
  const executable = await downloadAndUnzipVSCode({ version: process.env.VSCODE_VERSION ?? "stable", cachePath: join(extension, ".vscode-test") });
  // VS Code is launched directly (not through `runTests`, which always disables workspace
  // trust) so a stalled window can be killed. A test window is never focused, so the
  // Chromium switches keep a background or covered window from being throttled.
  const args = [
    fixture.repo,
    "--disable-extensions",
    "--skip-welcome",
    "--skip-release-notes",
    "--no-sandbox",
    "--disable-gpu-sandbox",
    "--disable-updates",
    "--no-cached-data",
    "--disable-renderer-backgrounding",
    "--disable-backgrounding-occluded-windows",
    // No OS keychain: on a CI runner a keychain access prompt that nobody answers blocks
    // the workbench before any view resolves.
    "--use-inmemory-secretstorage",
    "--use-mock-keychain",
    "--user-data-dir",
    join(profile, "user"),
    "--extensions-dir",
    join(profile, "extensions"),
    ...trust,
    `--extensionTestsPath=${join(__dirname, "suite/index.js")}`,
    `--extensionDevelopmentPath=${extension}`,
  ];
  await spawnBounded(executable, args, { ...process.env, ...env }, suite);
}

/** Run VS Code until it exits; kill it and fail when the suite exceeds its time limit. */
function spawnBounded(executable: string, args: string[], env: NodeJS.ProcessEnv, suite: string): Promise<void> {
  const limit = Number(process.env.CYBER_TEST_SUITE_TIMEOUT_MS ?? 8 * 60 * 1000);
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, { env, stdio: ["ignore", "pipe", "pipe"] });
    child.stdout?.on("data", (chunk: Buffer) => process.stdout.write(chunk));
    child.stderr?.on("data", (chunk: Buffer) => process.stderr.write(chunk));
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`the ${suite} suite did not finish within ${limit} ms; VS Code was killed`));
    }, limit);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("exit", (code) => {
      clearTimeout(timer);
      if (code === 0) resolve();
      else reject(new Error(`the ${suite} suite failed with exit code ${code}`));
    });
  });
}

/**
 * On macOS, App Nap throttles a background VS Code until API calls such as
 * `openTextDocument` stall for a minute or more. CI runners never focus the test window,
 * so `CYBER_TEST_DISABLE_APP_NAP=1` turns App Nap off for the VS Code bundle first.
 */
function disableAppNap(): void {
  if (!foreground()) return;
  spawnSync("defaults", ["write", "com.microsoft.VSCode", "NSAppSleepDisabled", "-bool", "YES"]);
}

function foreground(): boolean {
  return process.platform === "darwin" && process.env.CYBER_TEST_DISABLE_APP_NAP === "1";
}


async function main(): Promise<void> {
  disableAppNap();
  const binary = process.env.CYBER_E2E_BINARY ?? join(repository, "target/debug/cyber");
  const fixture = new Fixture(binary);
  const profile = await mkdtemp(join(process.platform === "darwin" ? "/tmp" : tmpdir(), "cyvp-"));
  let failed = false;
  try {
    await fixture.create();
    await fixture.startServer();
    const created = fixture.cli(["ide", "token", "create", "--client", "vscode"]);
    if (created.status !== 0) throw new Error(`cyber ide token create failed: ${created.stderr}`);
    const token = (await readFile(join(fixture.home, "state/ide/vscode.token"), "utf8")).split("\n")[0] ?? "";
    await launch(fixture, "chat", profile);
    await launch(fixture, "restricted", profile);
    const tokens = [token, (await readFile(join(fixture.home, "state/ide/vscode.token"), "utf8").catch(() => "")).split("\n")[0] ?? ""].filter(Boolean);
    for (const value of tokens) {
      const leaked = await holding(profile, value);
      if (leaked.length > 0) throw new Error(`the IDE token was persisted in: ${leaked.join(", ")}`);
    }
    console.log("No IDE token in the VS Code profile (settings, storage, logs).");
  } catch (error) {
    failed = true;
    console.error(error);
  } finally {
    await fixture.close();
    await rm(profile, { recursive: true, force: true });
  }
  process.exit(failed ? 1 : 0);
}

// A stalled VS Code (a workbench that never opens editors) must fail the run, not hang
// the job: the whole run is bounded.
const limit = Number(process.env.CYBER_TEST_TIMEOUT_MS ?? 20 * 60 * 1000);
setTimeout(() => {
  console.error(`the integration run exceeded ${limit} ms`);
  process.exit(1);
}, limit).unref();

void main();
