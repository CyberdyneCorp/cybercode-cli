/** Experimental fixture staging only. Production package admission, immutable
 * ownership and signature/entitlement compatibility still require review. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, constants, copyFileSync, readFileSync, realpathSync } from "node:fs";
import { createHash, randomUUID } from "node:crypto";
import { isAbsolute } from "node:path";

export function stageNative(source: string, destination: string) {
  assert.equal(process.platform, "darwin");
  assert(isAbsolute(source) && isAbsolute(destination) && source !== destination);
  copyFileSync(source, destination, constants.COPYFILE_EXCL);
  const run = (program: string, args: string[]) => {
    const result = spawnSync(program, args, {
      encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024,
      env: { PATH: "/usr/bin:/bin:/usr/sbin:/sbin" },
    });
    if (result.error) throw result.error;
    assert.equal(result.status, 0, `${program} failed: ${result.stderr}`);
    return result.stdout + result.stderr;
  };
  const architectures = run("/usr/bin/lipo", ["-archs", destination]).trim().split(/\s+/);
  assert(architectures.length > 0 && architectures.length <= 4 &&
    architectures.every(arch => /^(arm64e?|x86_64h?)$/.test(arch)), "unsupported native fixture architecture");
  // A fresh owned identifier also changes each CodeDirectory hash: an existing
  // ad-hoc image must not retain a platform trust-cache identity after staging.
  const identifier = `cyber.proof.staged.${randomUUID()}`;
  run("/usr/bin/codesign", ["--force", "--sign", "-", "--identifier", identifier, "--timestamp=none", destination]);
  run("/usr/bin/codesign", ["--verify", "--strict", "--all-architectures", "-R",
    `=identifier "${identifier}" and !(anchor apple)`, destination]);
  for (const arch of architectures) {
    const details = run("/usr/bin/codesign", ["--display", "--verbose=4", "--arch", arch, destination]);
    assert(details.includes("Signature=adhoc"), `staged ${arch} fixture must have an ad-hoc signature`);
  }
  chmodSync(destination, 0o500);
  return { executable: realpathSync(destination),
    source_sha256: createHash("sha256").update(readFileSync(source)).digest("hex"),
    staged_sha256: createHash("sha256").update(readFileSync(destination)).digest("hex"),
    signature: "adhoc", identifier, architectures, all_architectures_verified: true };
}
