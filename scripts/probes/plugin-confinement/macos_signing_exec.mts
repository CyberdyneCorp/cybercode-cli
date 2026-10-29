/** Experimental subject-identity exec admission. This tests a candidate and its
 * collision counterexample; it is not production package admission. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { copyFileSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { release, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const source = dirname(fileURLToPath(import.meta.url));
const environment = { PATH: "/usr/bin:/bin:/usr/sbin:/sbin" };
function run(program: string, args: string[]) {
  const result = spawnSync(program, args, { env: environment, encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024 });
  if (result.error) throw result.error;
  return { status: result.status, signal: result.signal, stdout: result.stdout, stderr: result.stderr };
}
function success(program: string, args: string[]) {
  const result = run(program, args);
  assert.equal(result.status, 0, JSON.stringify({ program, ...result }));
  return result;
}
function sign(program: string, identifier: string) {
  success("/usr/bin/codesign", ["--force", "--sign", "-", "--identifier", identifier, "--timestamp=none", program]);
  return success("/usr/bin/codesign", ["--verify", "--strict", "--all-architectures", program]);
}
function embeddedSignature(program: string) {
  const bytes = readFileSync(program);
  assert.equal(bytes.readUInt32LE(0), 0xfeedfacf, "controlled compiler fixture must be thin little-endian Mach-O 64");
  let offset = 32;
  for (let i = 0; i < bytes.readUInt32LE(16); i++) {
    const size = bytes.readUInt32LE(offset + 4);
    assert(size >= 8 && offset + size <= bytes.length);
    if (bytes.readUInt32LE(offset) === 0x1d) {
      assert.equal(size, 16);
      const start = bytes.readUInt32LE(offset + 8), length = bytes.readUInt32LE(offset + 12);
      assert(start >= offset + size && start + length <= bytes.length);
      return { prefix: bytes.subarray(0, start), blob: bytes.subarray(start, start + length) };
    }
    offset += size;
  }
  throw new Error("controlled image has no embedded signature");
}

assert.equal(process.platform, "darwin", "this experiment requires macOS");
const { values } = parseArgs({ options: { report: { type: "string" } } });
const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-macos-signing-exec-")));
try {
  const launcher = join(root, "trusted-launcher"), fixture = join(root, "untrusted-executable");
  const mutation = join(root, "signature-mutation"), descriptors = join(root, "descriptor-fixture");
  for (const [input, output] of [["macos_exec.c", launcher], ["fixture_exec.c", fixture], ["macos_signature.c", mutation], ["macos_fds.c", descriptors]]) {
    success("/usr/bin/xcrun", ["clang", "-O1", "-Wall", "-Wextra", "-Werror", "-Wno-error=deprecated-declarations", join(source, input), "-o", output]);
  }
  const nonce = randomUUID();
  const helperID = `com.cyberdyne.probe.helperX.${nonce}`, targetID = `com.cyberdyne.probe.payload.${nonce}`;
  assert.equal(helperID.length, targetID.length);
  const signature = sign(launcher, helperID);
  sign(fixture, targetID);
  sign(mutation, targetID);
  // Broad reads isolate exec identity. This is deliberately not an fs.read policy.
  const base = '(version 1)(deny default)(allow file-read* file-map-executable)(allow sysctl-read)(allow signal (target same-sandbox))(allow process-info* (target same-sandbox))';
  function profile(target: string) {
    const file = join(root, `policy-${randomUUID()}.sb`);
    writeFileSync(file, base + `(allow process-exec (require-all (signing-identifier ${JSON.stringify(helperID)}) (literal ${JSON.stringify(target)})))`, { mode: 0o600 });
    return file;
  }
  const nativeControl = JSON.parse(success(launcher, ["-", fixture]).stdout);
  assert.deepEqual(nativeControl, { initial_exec: true, self_exec: true });
  const native = JSON.parse(success(launcher, [profile(fixture), fixture]).stdout);
  assert.deepEqual(native, { initial_exec: true, self_exec: false });
  const platformArgs = ["-c", "printf 'initial-admitted\\n'; exec /bin/bash -c 'printf self-exec'"];
  const platformHash = createHash("sha256").update(readFileSync("/bin/bash")).digest("hex");
  const platformControl = success(launcher, ["-", "/bin/bash", ...platformArgs]);
  assert.equal(platformControl.stdout, "initial-admitted\nself-exec");
  const platform = run(launcher, [profile("/bin/bash"), "/bin/bash", ...platformArgs]);
  assert.equal(platform.signal, null, "a killed image does not prove admission or denial");
  assert(platform.status !== null && platform.status > 0);
  assert.equal(platform.stdout, "initial-admitted\n");
  assert.match(platform.stderr, /Operation not permitted|Permission denied/);
  assert.equal(createHash("sha256").update(readFileSync("/bin/bash")).digest("hex"), platformHash);
  // A matching identifier is a real counterexample, so admission cannot rely
  // on this predicate without proving immutable target/helper identity separation.
  const collision = join(root, "identifier-collision");
  copyFileSync(fixture, collision);
  sign(collision, helperID);
  const collisionResult = JSON.parse(success(launcher, [profile(collision), collision]).stdout);
  assert.deepEqual(collisionResult, { initial_exec: true, self_exec: true });
  // Try attaching a different, valid ad-hoc signature through an O_RDONLY FD.
  // Prefix equality proves the alternate signature covers exactly the same code.
  const alternate = join(root, "alternate-signature-image");
  copyFileSync(mutation, alternate);
  sign(alternate, helperID);
  const originalSignature = embeddedSignature(mutation), forgedSignature = embeddedSignature(alternate);
  assert.deepEqual(originalSignature.prefix, forgedSignature.prefix, "alternate signature must cover unchanged code");
  assert(!originalSignature.blob.equals(forgedSignature.blob));
  const detached = join(root, "alternate-signature.blob");
  writeFileSync(detached, forgedSignature.blob, { mode: 0o600 });
  const mutationHash = createHash("sha256").update(readFileSync(mutation)).digest("hex");
  const mutationControl = success(launcher, ["-", mutation, detached]).stdout.trim().split("\n").map(line => JSON.parse(line));
  assert.equal(mutationControl.length, 2);
  assert.equal(mutationControl[0].initial_exec, true);
  assert.deepEqual(mutationControl[1], { self_exec: true });
  const mutationResult = success(launcher, [profile(mutation), mutation, detached]).stdout.trim().split("\n").map(line => JSON.parse(line));
  assert.equal(mutationResult.length, 2);
  assert.equal(mutationResult[0].initial_exec, true);
  assert.equal(mutationResult[1].self_exec, false, "attaching a signature must not recover bootstrap exec permission");
  assert.equal(createHash("sha256").update(readFileSync(mutation)).digest("hex"), mutationHash);
  const ownedFile = join(root, "dummy-private-descriptor");
  writeFileSync(ownedFile, "fixture only", { mode: 0o600 });
  const descriptorControl = JSON.parse(success(descriptors, ["-", fixture, ownedFile]).stdout);
  const descriptorClosed = JSON.parse(success(descriptors, [launcher, fixture, ownedFile]).stdout);
  assert.deepEqual(descriptorControl, { inherited_descriptor: true });
  assert.deepEqual(descriptorClosed, { inherited_descriptor: false });
  const sources = ["macos_signing_exec.mts", "macos_exec.c", "fixture_exec.c", "macos_signature.c", "macos_fds.c"];
  const digest = createHash("sha256");
  for (const file of sources) digest.update(file + "\0").update(readFileSync(join(source, file)));
  const report = { recorded_at: new Date().toISOString(), platform: process.platform, architecture: process.arch, kernel_release: release(),
    sources, source_sha256: digest.digest("hex"), signing_identifier: helperID, signature, native_control: nativeControl, native,
    original_platform_image: { path: "/bin/bash", sha256: platformHash, control: platformControl, restricted: platform },
    same_identifier_counterexample: collisionResult, detached_signature: { control: mutationControl, restricted: mutationResult, original_image_unchanged: true },
    inherited_above_soft_limit: { control: descriptorControl, launcher: descriptorClosed },
    limitation: "Experimental exec-only candidate. Identifier collision is an observed bypass. Immutable package/helper identity admission across all slices, other kernel mutation interfaces, packaging and owner integration remain unproved; no production support change." };
  if (values.report) writeFileSync(values.report, JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
  console.log(JSON.stringify(report, null, 2));
} finally { rmSync(root, { recursive: true, force: true }); }
