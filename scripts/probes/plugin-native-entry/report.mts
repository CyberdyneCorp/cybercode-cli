/** Source/build provenance only. Test outcomes remain in the associated job log. */
import { createHash } from "node:crypto";
import { createReadStream, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { parseArgs } from "node:util";

const { values } = parseArgs({ options: { binary: { type: "string" }, out: { type: "string" } } });
if (!values.binary || !values.out) throw new Error("--binary and --out are required");
const sources = [
  "crates/cyber-sandbox/native/linux_plugin_exec.c", "crates/cyber-sandbox/native/linux_filter.h",
  "crates/cyber-sandbox/native/linux_initial_exec.h", "crates/cyber-sandbox/tests/support/native_exec_linux.rs",
  "Cargo.lock", "crates/cyber-sandbox/Cargo.toml", "crates/cyber-sandbox/build.rs",
  "crates/cyber-sandbox/native/macos_plugin_exec.c", "crates/cyber-sandbox/src/native_exec.rs",
  "crates/cyber-sandbox/src/bin/cyber-sandbox-exec.rs", "crates/cyber-sandbox/tests/native_exec.rs",
  "crates/cyber-sandbox/tests/fixtures/plugin_native_fds.c",
  "scripts/probes/plugin-confinement/fixture_exec.c", "scripts/probes/plugin-confinement/fixture_native.c",
  "scripts/probes/plugin-confinement/process_policy.h", "scripts/probes/plugin-native-entry/report.mts",
];
const sourceHash = createHash("sha256");
for (const source of sources) sourceHash.update(source + "\0").update(readFileSync(source));
const binary = resolve(values.binary);
let image: { sha256: string; bytes: number } | null = null;
if (existsSync(binary)) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(binary)) hash.update(chunk);
  image = { sha256: hash.digest("hex"), bytes: statSync(binary).size };
}
const report = {
  recorded_at: new Date().toISOString(), platform: process.platform, architecture: process.arch,
  checkout: process.env.GITHUB_SHA ?? null, run_id: process.env.GITHUB_RUN_ID ?? null,
  sources, source_sha256: sourceHash.digest("hex"), packaged_binary: image,
  scope: "Build provenance. macOS tests execute private copies signed with fresh identifiers; read the job for outcomes. This does not establish plugin launch admission or runtime compatibility beyond the tested fixtures.",
};
mkdirSync(dirname(resolve(values.out)), { recursive: true });
writeFileSync(values.out, JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
