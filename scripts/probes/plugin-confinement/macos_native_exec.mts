/** Evidence for insufficient pre-exec Seatbelt strategies, not production
 * support and not a proof that no other mechanism can work. */
import assert from "node:assert/strict";
import { stageNative } from "./macos_stage.mts";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { appendFileSync, copyFileSync, linkSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
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
  assert.equal(result.status, 0, `${program}: ${JSON.stringify({ status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr })}`);
  return result.stdout.trim();
}

assert.equal(process.platform, "darwin", "this initial-exec experiment requires macOS");
const { values } = parseArgs({ options: { report: { type: "string" } } });
const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-macos-native-exec-")));
try {
  const compiler = success("/usr/bin/xcrun", ["--find", "clang"]);
  const sdk = success("/usr/bin/xcrun", ["--show-sdk-path"]);
  const fixture = join(root, "untrusted-executable"), launcher = join(root, "trusted-launcher"), descriptors = join(root, "descriptor-fixture");
  for (const [input, output] of [["fixture_exec.c", fixture], ["macos_exec.c", launcher], ["macos_fds.c", descriptors]]) {
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
  // An existing writable input alias must not become an alias of the private
  // staged inode. Mutate only a separate controlled copy, never a system file.
  const aliasedInput = join(root, "aliased-native-input"), inputAlias = join(root, "writable-input-alias");
  copyFileSync(fixture, aliasedInput);
  linkSync(aliasedInput, inputAlias);
  const aliasStage = stageNative(aliasedInput, join(root, "staged-aliased-input"));
  const originalInput = statSync(aliasedInput, { bigint: true }), stagedInput = statSync(aliasStage.executable, { bigint: true });
  assert.equal(originalInput.nlink, 2n);
  assert.equal(stagedInput.nlink, 1n);
  assert(originalInput.dev !== stagedInput.dev || originalInput.ino !== stagedInput.ino);
  appendFileSync(inputAlias, "changed through pre-existing input alias");
  const stagedUnchanged = createHash("sha256").update(readFileSync(aliasStage.executable)).digest("hex") === aliasStage.staged_sha256;
  assert(stagedUnchanged, "input alias mutation must not change the staged image");
  const aliasControl = JSON.parse(success("/usr/bin/sandbox-exec", ["-p", base + "(allow process-exec)", aliasStage.executable]));
  assert.deepEqual(aliasControl, { initial_exec: true, self_exec: true });
  const platformExecutable = "/bin/bash";
  const platformSelfExec = success("/usr/bin/sandbox-exec", ["-p", platformProfile(platformExecutable),
    platformExecutable, "-c", "exec /bin/bash -c 'printf platform-self-exec'"]);
  assert.equal(platformSelfExec, "platform-self-exec", "platform payload must demonstrate the retained exec exception");
  // The staged copy deliberately discards the platform signature. Check every
  // architecture slice, then repeat the counterexample through the actual rule.
  const stagedPlatform = stageNative(platformExecutable, join(root, "staged-platform-payload"));
  const stagedArgs = [stagedPlatform.executable, "-c", "printf 'initial-admitted\\n'; exec \"$0\" -c 'printf platform-self-exec'"];
  console.error(JSON.stringify({ experiment: "staged-platform-positive-control",
    platform: process.platform, architecture: process.arch, kernel_release: release(), staging: stagedPlatform }));
  const stagedDirect = run(stagedPlatform.executable, stagedArgs.slice(1));
  const stagedSandbox = run("/usr/bin/sandbox-exec", ["-p", base + "(allow process-exec)", ...stagedArgs]);
  const outcome = (result: ReturnType<typeof run>) => ({ status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr });
  const stagingControls = { direct: outcome(stagedDirect), sandbox: outcome(stagedSandbox) };
  console.error(JSON.stringify({ experiment: "staged-platform-controls", ...stagingControls }));
  assert.equal(stagedDirect.status, 0, `staged direct positive control: ${JSON.stringify(stagingControls)}`);
  assert.equal(stagedSandbox.status, 0, `staged sandbox positive control: ${JSON.stringify(stagingControls)}`);
  const stagedControl = stagedSandbox.stdout.trim();
  assert.equal(stagedDirect.stdout.trim(), "initial-admitted\nplatform-self-exec");
  assert.equal(stagedControl, "initial-admitted\nplatform-self-exec");
  const stagedRestricted = run("/usr/bin/sandbox-exec", ["-p", platformProfile(stagedPlatform.executable), ...stagedArgs]);
  assert.equal(stagedRestricted.signal, null, "termination is not a policy-denied exec result");
  assert(stagedRestricted.status !== null && stagedRestricted.status > 0,
    `bash must report the rejected exec: ${JSON.stringify(outcome(stagedRestricted))}`);
  assert.match(stagedRestricted.stderr, /Operation not permitted|Permission denied/,
    "exec denial must report a native permission error");
  assert.equal(stagedRestricted.stdout.trim(), "initial-admitted");
  assert(!stagedRestricted.stderr.includes("sandbox_init"), "policy startup failure is not exec denial evidence");
  const ownedFile = join(root, "dummy-private-descriptor");
  writeFileSync(ownedFile, "fixture only", { mode: 0o600 });
  const descriptorControl = JSON.parse(success(descriptors, ["-", fixture, ownedFile]));
  assert.deepEqual(descriptorControl, { inherited_descriptor: true });
  const descriptorClosed = JSON.parse(success(descriptors, [launcher, fixture, ownedFile]));
  assert.deepEqual(descriptorClosed, { inherited_descriptor: false }, "descriptor above the lowered soft limit must be closed");
  const sources = ["macos_native_exec.mts", "macos_exec.c", "fixture_exec.c", "macos_stage.mts", "macos_fds.c"];


  const digest = createHash("sha256");
  for (const file of sources) digest.update(file + "\0").update(readFileSync(join(source, file)));
  const report = { recorded_at: new Date().toISOString(), platform: process.platform, architecture: process.arch, kernel_release: release(),
    sources, source_sha256: digest.digest("hex"), control,
    no_exec_policy: { initial_exec: false, exit_status: denied.status }, literal_exception: literal,
    platform_attribute: { custom_native: platformNative,
      platform_executable: platformExecutable, platform_self_exec: true,
      limitation: "The platform-subject exception is not one-shot for a platform executable; an independently enforced target-admission contract remains unproved." },
    inherited_above_soft_limit: { descriptor: 500, soft_limit: 64, control: descriptorControl, launcher: descriptorClosed },
    staged_input_alias: { staging: aliasStage, source_link_count: 2, staged_link_count: 1, distinct_inode: true,
      input_alias_mutated: true, staged_sha256_unchanged: stagedUnchanged, control: aliasControl },
    staged_platform: { ...stagedPlatform, controls: stagingControls, restricted: outcome(stagedRestricted), control_self_exec: true, initial_exec: true, self_exec: false,
      limitation: "Fixture staging resolves this counterexample locally; production immutable-package admission, transformed-image integrity, entitlement compatibility and architecture acceptance remain unproved." },
    conclusion: "The platform predicate alone is insufficient. An ad-hoc re-signed fixture copy starts and denies self-exec, including a formerly platform-signed input; this is an experimental candidate, not production admission or an fs.read policy." };
  if (values.report) writeFileSync(values.report, JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
  console.log(JSON.stringify(report, null, 2));
} finally { rmSync(root, { recursive: true, force: true }); }
