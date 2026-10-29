/** Controlled admission counterexamples, not production confinement. A path-only
 * policy cannot protect objects that already have aliases in admitted roots. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { copyFileSync, existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { release, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const source = dirname(fileURLToPath(import.meta.url));
const probes = dirname(source);
const environment = { PATH: "/usr/bin:/bin:/usr/sbin:/sbin" };
function run(program: string, args: string[]) {
  const result = spawnSync(program, args, { env: environment, encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024 });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, JSON.stringify({ program, status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr }));
  return result.stdout;
}
function hash(path: string) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}
function distinct(left: string, right: string) {
  const a = statSync(left, { bigint: true }), b = statSync(right, { bigint: true });
  return a.dev !== b.dev || a.ino !== b.ino;
}
// This deliberately small guard checks known files only, using stat without
// opening private regular files. It is not a complete private-tree inventory,
// durable identity history, race-proof package builder or live host admission.
function admit(privateFiles: string[], stagedFiles: string[]) {
  for (const [files, reason] of [[privateFiles, "private file has pre-existing aliases"], [stagedFiles, "staged image has pre-existing aliases"]] as const) {
    for (const file of files) {
      const metadata = statSync(file, { bigint: true });
      assert(metadata.isFile() && metadata.nlink === 1n, reason);
    }
  }
}

assert.equal(process.platform, "darwin", "this admission experiment requires macOS");
const { values } = parseArgs({ options: { report: { type: "string" } } });
const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-plugin-admission-")));
try {
  const helper = join(root, "trusted-launcher"), fixture = join(root, "untrusted-fixture");
  for (const [input, output] of [[join(probes, "plugin-confinement/macos_exec.c"), helper], [join(source, "fixture.c"), fixture]]) {
    run("/usr/bin/xcrun", ["clang", "-O1", "-Wall", "-Wextra", "-Werror", "-Wno-error=deprecated-declarations", input, "-o", output]);
  }
  const identifier = `com.cyberdyne.probe.admission.${randomUUID()}`;
  run("/usr/bin/codesign", ["--force", "--sign", "-", "--identifier", identifier, "--timestamp=none", helper]);
  run("/usr/bin/codesign", ["--verify", "--strict", "--all-architectures", helper]);
  // arm64 clang normally emits an ad-hoc signature; Intel clang need not.
  // Prepare a signed controlled input on both hosts before testing unchanged
  // copies. The copy step must not repair or replace its input signature.
  run("/usr/bin/codesign", ["--force", "--sign", "-", "--identifier", `com.cyberdyne.probe.fixture.${randomUUID()}`, "--timestamp=none", fixture]);
  run("/usr/bin/codesign", ["--verify", "--strict", "--all-architectures", fixture]);
  const privateRoot = join(root, "private"), packageRoot = join(root, "input-package"), scratch = join(root, "scratch"), staged = join(root, "staged-package");
  for (const path of [privateRoot, packageRoot, scratch, staged]) mkdirSync(path, { mode: 0o700 });
  const secret = join(privateRoot, "dummy-private"), data = join(packageRoot, "data");
  const privateAlias = join(packageRoot, "existing-private-alias"), writableAlias = join(scratch, "existing-package-alias"), receipt = join(scratch, "started");
  writeFileSync(secret, "dummy private fixture", { mode: 0o600 });
  writeFileSync(data, "dummy package fixture", { mode: 0o600 });
  linkSync(secret, privateAlias);
  linkSync(data, writableAlias);
  function profile(executable: string, packageDirectory: string) {
    const rules = ['(version 1)', '(deny default)', '(allow file-read-metadata)', '(allow sysctl-read)',
      '(allow signal (target same-sandbox))', '(allow process-info* (target same-sandbox))',
      '(allow file-read-data (literal "/"))', '(allow file-read* file-write* (literal "/dev/null"))'];
    for (const path of ["/System", "/usr/lib", "/bin", "/usr/bin", packageDirectory, scratch]) {
      rules.push(`(allow file-read* file-map-executable (subpath ${JSON.stringify(path)}))`);
    }
    rules.push(`(allow file-read* file-map-executable (literal ${JSON.stringify(executable)}))`,
      `(allow file-write* (subpath ${JSON.stringify(scratch)}))`,
      `(deny file-read* file-write* (subpath ${JSON.stringify(privateRoot)}))`,
      `(allow process-exec (require-all (signing-identifier ${JSON.stringify(identifier)}) (literal ${JSON.stringify(executable)})))`);
    const file = join(root, `policy-${randomUUID()}.sb`);
    writeFileSync(file, rules.join("\n"), { mode: 0o600 });
    return file;
  }
  function execute(policy: string, executable: string, read: string, write: string) {
    const result = JSON.parse(run(helper, [policy, executable, read, write, receipt]));
    assert.equal(readFileSync(receipt, "utf8"), "started");
    return result;
  }
  const bypass = { initial_exec: true, private_alias_read: true, package_alias_write: true };
  const control = execute("-", fixture, privateAlias, writableAlias);
  assert.deepEqual(control, bypass);
  writeFileSync(data, "dummy package fixture");
  const pathOnly = execute(profile(fixture, packageRoot), fixture, privateAlias, writableAlias);
  console.error(JSON.stringify({ experiment: "pre-existing-alias-controls", platform: process.platform,
    architecture: process.arch, kernel_release: release(), control, path_only_counterexample: pathOnly }));
  assert.deepEqual(pathOnly, bypass, "pre-existing aliases must demonstrate the path-only policy gap");
  assert.equal(readFileSync(data, "utf8"), "alias mutation");
  unlinkSync(receipt);
  assert.throws(() => {
    admit([secret], []);
    execute(profile(fixture, packageRoot), fixture, privateAlias, writableAlias);
  }, /private file has pre-existing aliases/);
  assert(!existsSync(receipt), "unsafe private binding must refuse before target startup");
  unlinkSync(privateAlias);
  writeFileSync(data, "dummy package fixture");
  const stagedData = join(staged, "data"), executable = join(staged, "native-fixture");
  copyFileSync(data, stagedData);
  copyFileSync(fixture, executable);
  assert(distinct(data, stagedData) && distinct(fixture, executable));
  assert.equal(hash(fixture), hash(executable));
  run("/usr/bin/codesign", ["--verify", "--strict", "--all-architectures", executable]);
  const stageHash = hash(stagedData);
  writeFileSync(writableAlias, "changed original input through its existing alias");
  assert.equal(hash(stagedData), stageHash, "source aliases must not mutate the staged package");
  const stageAlias = join(scratch, "existing-staged-alias");
  linkSync(executable, stageAlias);
  assert.throws(() => {
    admit([secret], [stagedData, executable]);
    execute(profile(executable, staged), executable, secret, stagedData);
  }, /staged image has pre-existing aliases/);
  assert(!existsSync(receipt));
  unlinkSync(stageAlias);
  admit([secret], [stagedData, executable]);
  const admitted = execute(profile(executable, staged), executable, secret, stagedData);
  assert.deepEqual(admitted, { initial_exec: true, private_alias_read: false, package_alias_write: false });
  assert.equal(hash(stagedData), stageHash);
  assert.equal(hash(fixture), hash(executable));
  const sources = ["plugin-admission/run.mts", "plugin-admission/fixture.c", "plugin-confinement/macos_exec.c"];
  const digest = createHash("sha256");
  for (const file of sources) digest.update(file + "\0").update(readFileSync(join(probes, file)));
  const report = { recorded_at: new Date().toISOString(), platform: process.platform, architecture: process.arch, kernel_release: release(),
    sources, source_sha256: digest.digest("hex"), control, path_only_counterexample: pathOnly,
    known_private_alias_refused_before_start: true, staged_executable_alias_refused_before_start: true,
    copied_package_distinct_inode: true, copied_executable_distinct_inode: true, input_alias_cannot_mutate_copy: true,
    executable_signature_preserved: true, admitted,
    limitation: "Controlled known-file stat guard and immutable-copy experiment, not production admission. Complete private-root inventory, directory aliases, concurrent replacement, generation ownership, budgets, cross-platform behavior and owner/SQLite integration remain required." };
  if (values.report) writeFileSync(values.report, JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
  console.log(JSON.stringify(report, null, 2));
} finally { rmSync(root, { recursive: true, force: true }); }
