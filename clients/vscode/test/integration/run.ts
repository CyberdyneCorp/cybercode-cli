/**
 * Launches VS Code with a temporary profile against the fixture (`@vscode/test-electron`),
 * twice: a trusted workspace for the chat suite, and Restricted Mode. Afterwards it checks
 * that the IDE token appears nowhere in the profile (settings, extension storage, logs)
 * and removes everything.
 */
import { readdir, readFile, mkdir, mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { downloadAndUnzipVSCode, runTests } from "@vscode/test-electron";
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

async function launch(fixture: Fixture, suite: "chat" | "restricted", profile: string): Promise<void> {
  const report = join(profile, `${suite}.report`);
  try {
    await runSuite(fixture, suite, profile, report);
  } finally {
    console.log(await readFile(report, "utf8").catch(() => "(no test report)"));
  }
}

async function runSuite(fixture: Fixture, suite: "chat" | "restricted", root: string, report: string): Promise<void> {
  const profile = join(root, suite);
  const trust = suite === "chat" ? ["--disable-workspace-trust"] : [];
  if (suite === "restricted") {
    // A fresh profile that has trusted nothing, opening untrusted folders in Restricted
    // Mode without the startup prompt.
    await mkdir(join(profile, "user/User"), { recursive: true });
    await writeFile(join(profile, "user/User/settings.json"), JSON.stringify({ "security.workspace.trust.enabled": true, "security.workspace.trust.startupPrompt": "never", "security.workspace.trust.untrustedFiles": "open" }));
  }
  const options = {
    version: process.env.VSCODE_VERSION ?? "stable",
    cachePath: join(extension, ".vscode-test"),
    extensionDevelopmentPath: extension,
    extensionTestsPath: join(__dirname, "suite/index.js"),
    launchArgs: [fixture.repo, "--disable-extensions", "--skip-welcome", "--skip-release-notes", "--disable-renderer-backgrounding", "--disable-backgrounding-occluded-windows", "--user-data-dir", join(profile, "user"), "--extensions-dir", join(profile, "extensions"), ...trust],
    extensionTestsEnv: { ...fixture.env, CYBER_VSCODE_TEST: "1", CYBER_TEST_SUITE: suite, CYBER_TEST_FIXTURE: JSON.stringify(fixture.info()), CYBER_TEST_REPORT: report },
  };
  if (suite === "chat") {
    await runTests(options);
    return;
  }
  // `runTests` always adds `--disable-workspace-trust`; Restricted Mode needs VS Code
  // launched without it.
  const executable = await downloadAndUnzipVSCode({ version: options.version, cachePath: options.cachePath });
  const args = [...options.launchArgs, "--no-sandbox", "--disable-gpu-sandbox", "--disable-updates", "--no-cached-data", `--extensionTestsPath=${options.extensionTestsPath}`, `--extensionDevelopmentPath=${extension}`];
  const code = await new Promise<number | null>((resolve, reject) => {
    const child = spawn(executable, args, { env: { ...process.env, ...options.extensionTestsEnv }, stdio: "inherit" });
    child.once("error", reject);
    child.once("exit", resolve);
  });
  if (code !== 0) throw new Error(`the ${suite} suite failed with exit code ${code}`);
}

async function main(): Promise<void> {
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

void main();
