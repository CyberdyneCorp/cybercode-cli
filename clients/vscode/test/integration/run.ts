/**
 * Launches VS Code with a temporary profile against the fixture (`@vscode/test-electron`),
 * twice: a trusted workspace for the chat suite, and Restricted Mode. Afterwards it checks
 * that the IDE token appears nowhere in the profile (settings, extension storage, logs)
 * and removes everything.
 */
import { readdir, readFile, mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { runTests } from "@vscode/test-electron";
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

async function runSuite(fixture: Fixture, suite: "chat" | "restricted", profile: string, report: string): Promise<void> {
  const trust = suite === "chat" ? ["--disable-workspace-trust"] : [];
  await runTests({
    version: process.env.VSCODE_VERSION ?? "stable",
    cachePath: join(extension, ".vscode-test"),
    extensionDevelopmentPath: extension,
    extensionTestsPath: join(__dirname, "suite/index.js"),
    launchArgs: [fixture.repo, "--disable-extensions", "--skip-welcome", "--skip-release-notes", "--disable-renderer-backgrounding", "--disable-backgrounding-occluded-windows", "--user-data-dir", join(profile, "user"), "--extensions-dir", join(profile, "extensions"), ...trust],
    extensionTestsEnv: { ...fixture.env, CYBER_VSCODE_TEST: "1", CYBER_TEST_SUITE: suite, CYBER_TEST_FIXTURE: JSON.stringify(fixture.info()), CYBER_TEST_REPORT: report },
  });
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
