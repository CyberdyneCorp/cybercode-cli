/** Evidence for insufficient pre-exec Seatbelt strategies, not production
 * support and not a proof that no other mechanism can work. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { release, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const source = dirname(fileURLToPath(import.meta.url));
const environment = { PATH: "/usr/bin:/bin:/usr/sbin:/sbin" };
function run(program: string, args: string[]) {
  const result = spawnSync(program, args, { env: environment, encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024 });
  if (result.error) throw result.error;
  return result;
}
function success(program: string, args: string[]) {
  const result = run(program, args);
  assert.equal(result.status, 0, `${program}: ${result.stderr}`);
  return result.stdout.trim();
}

assert.equal(process.platform, "darwin", "this initial-exec experiment requires macOS");
const { values } = parseArgs({ options: { report: { type: "string" } } });
const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-macos-native-exec-")));
try {
  const compiler = success("/usr/bin/xcrun", ["--find", "clang"]);
  const sdk = success("/usr/bin/xcrun", ["--show-sdk-path"]);
  const fixture = join(root, "untrusted-executable"), launcher = join(root, "trusted-launcher");
  for (const [input, output] of [["fixture_exec.c", fixture], ["macos_exec.c", launcher]]) {
    success(compiler, ["-isysroot", sdk, "-O1", "-Wall", "-Wextra", "-Werror", "-Wno-error=deprecated-declarations", join(source, input), "-o", output]);
  }
  const control = JSON.parse(success(launcher, ["-", fixture]));
  assert.deepEqual(control, { initial_exec: true, self_exec: true });
  // Broad read access deliberately isolates this exec experiment. It is not a
  // candidate fs.read policy and touches no owner/private fixture data.
  const base = '(version 1)(deny default)(allow file-read* file-map-executable)(allow sysctl-read)(allow signal (target same-sandbox))(allow process-info* (target same-sandbox))';
  const deniedProfile = join(root, "deny-exec.sb"), literalProfile = join(root, "literal-exec.sb");
  writeFileSync(deniedProfile, base, { mode: 0o600 });
  writeFileSync(literalProfile, base + `(allow process-exec (literal ${JSON.stringify(fixture)}))`, { mode: 0o600 });
  const denied = run(launcher, [deniedProfile, fixture]);
  assert.equal(denied.status, 13, `expected initial exec denial, got ${denied.status}: ${denied.stderr}`);
  assert.equal(denied.stdout, "");
  const literal = JSON.parse(success(launcher, [literalProfile, fixture]));
  assert.deepEqual(literal, { initial_exec: true, self_exec: true });
  // Unlike a broad literal exception, the platform-subject predicate permits
  // sandbox-exec's initial launch and denies this custom fixture's self-exec.
  // It is nevertheless not a general native boundary: a platform executable
  // keeps the subject predicate after its own initial launch and can self-exec.
  // Use the canonical private fixture path; /var and /private/var aliases can
  // otherwise turn a literal-path mismatch into a misleading initial refusal.
  const platformProfile = (executable: string) => base +
    `(allow process-exec (require-all (process-attribute is-platform-binary) (literal ${JSON.stringify(executable)})))`;
  const platformNative = JSON.parse(success("/usr/bin/sandbox-exec", ["-p", platformProfile(fixture), fixture]));
  assert.deepEqual(platformNative, { initial_exec: true, self_exec: false });
  const platformExecutable = "/bin/bash";
  const platformSelfExec = success("/usr/bin/sandbox-exec", ["-p", platformProfile(platformExecutable),
    platformExecutable, "-c", "exec /bin/bash -c 'printf platform-self-exec'"]);
  assert.equal(platformSelfExec, "platform-self-exec", "platform payload must demonstrate the retained exec exception");
  const sources = ["macos_native_exec.mts", "macos_exec.c", "fixture_exec.c"];
  const digest = createHash("sha256");
  for (const file of sources) digest.update(file + "\0").update(readFileSync(join(source, file)));
  const report = { recorded_at: new Date().toISOString(), platform: process.platform, architecture: process.arch, kernel_release: release(),
    sources, source_sha256: digest.digest("hex"), control,
    no_exec_policy: { initial_exec: false, exit_status: denied.status }, literal_exception: literal,
    platform_attribute: { custom_native: platformNative,
      platform_executable: platformExecutable, platform_self_exec: true,
      limitation: "The platform-subject exception is not one-shot for a platform executable; an independently enforced target-admission contract remains unproved." },
    conclusion: "No tested strategy proves the general independent initial/later-exec contract. The platform predicate distinguishes the custom fixture but retains exec rights for platform payloads; other mechanisms or independently proved target admission remain unproved. Broad reads here are not an fs.read policy." };
  if (values.report) writeFileSync(values.report, JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
  console.log(JSON.stringify(report, null, 2));
} finally { rmSync(root, { recursive: true, force: true }); }
