/** Native Linux mount-composition experiment; not plugin launch admission. */
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { release, tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";
import { parseArgs } from "node:util";

const actor = String.raw`
import errno, json, os, sys
from pathlib import Path
p = json.loads(sys.argv[1])
def attempt(action):
    try:
        action()
        return {"ok": True, "errno": None}
    except OSError as error:
        return {"ok": False, "errno": error.errno}
def rename_back(source, destination):
    os.rename(source, destination)
    os.rename(destination, source)
def read_kind(path):
    try:
        value = Path(path).read_bytes()
        return "empty" if value == b"" else "data"
    except OSError:
        return "denied"
renames = []
siblings = []
for label, root in p["roots"].items():
    parent = Path(root).parent
    for level in range(2):
        source = parent if level == 0 else parent.parent
        destination = source.with_name(source.name + "-moved")
        renames.append({"root": label, "level": level,
                        "result": attempt(lambda: rename_back(source, destination))})
        siblings.append(attempt(lambda: (source / "public-sibling").write_bytes(b"fixture-write")))
immutable = [attempt(lambda path=path: Path(path).write_bytes(b"must-not-write"))
             for path in p["immutable_data"]]
public = Path(p["project"]) / "public"
result = {"started": True, "renames": renames, "siblings": siblings,
          "immutable_writes": immutable,
          "private_directory_read": read_kind(p["private_data"]),
          "private_file_read": read_kind(p["roots"]["private_file"]),
          "private_writes": [attempt(lambda path=path: Path(path).write_bytes(b"must-not-write"))
                             for path in [p["private_data"], p["roots"]["private_file"]]],
          "symlink_private_read": read_kind(p["alias"]),
          "workspace_write": attempt(lambda: (Path(p["project"]) / "written").write_bytes(b"fixture-write")),
          "public_rename": attempt(lambda: rename_back(public, public.with_name("public-moved"))),
          "scratch_write": attempt(lambda: (Path(p["scratch"]) / "written").write_bytes(b"fixture-write"))}
print(json.dumps({"jsonrpc": "2.0", "id": "mount-proof", "result": result}))
`;

type Layout = {
  project: string; scratch: string; empty_mask: string; write: boolean;
  roots: Record<string, string>; immutable_data: string[];
  private_data: string; alias: string;
};
function beneath(path: string, parent: string): boolean {
  const rest = relative(parent, path);
  return rest !== "" && rest !== ".." && !rest.startsWith(`..${sep}`) && !isAbsolute(rest);
}
function anchors(layout: Layout): string[] {
  const paths = new Set<string>();
  for (const root of Object.values(layout.roots)) {
    for (let parent = dirname(root); beneath(parent, layout.project); parent = dirname(parent)) paths.add(parent);
  }
  return [...paths].sort((a, b) => a.split(sep).length - b.split(sep).length || a.localeCompare(b));
}
function fixture(root: string, write: boolean): Layout {
  const project = join(root, 'project "quote" \\ workspace');
  const scratch = join(root, "scratch");
  const empty_mask = join(root, "owned-empty-mask");
  writeFileSync(empty_mask, "", { mode: 0o000 });
  const roots = { package: join(project, "extensions/level/package"),
    runtime_file: join(project, "toolchain/level/runtime-file"),
    private_directory: join(project, ".owner/level/state"),
    private_file: join(project, ".secrets/level/key") };
  for (const path of [roots.package, dirname(roots.runtime_file), roots.private_directory,
    dirname(roots.private_file), scratch, join(project, "public")]) mkdirSync(path, { recursive: true, mode: 0o700 });
  const immutable_data = [join(roots.package, "data"), roots.runtime_file];
  const private_data = join(roots.private_directory, "data");
  for (const path of [...immutable_data, private_data, roots.private_file]) writeFileSync(path, "owned-fixture-only", { mode: 0o600 });
  // The symlink itself is created by the owner before the read-only package mount.
  const alias = join(roots.package, "alias");
  return { project, scratch, empty_mask, write, roots, immutable_data, private_data, alias };
}
function argumentsFor(layout: Layout, anchored: boolean, deviceMask: boolean): string[] {
  const args = ["--die-with-parent", "--unshare-user", "--unshare-pid", "--unshare-net",
    "--unshare-ipc", "--unshare-uts", "--disable-userns", "--cap-drop", "ALL", "--clearenv",
    "--setenv", "PATH", "/usr/bin:/bin", "--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev",
    layout.write ? "--bind" : "--ro-bind", layout.project, layout.project];
  if (anchored) for (const parent of anchors(layout)) args.push(layout.write ? "--bind" : "--ro-bind", parent, parent);
  for (const path of [layout.roots.package, layout.roots.runtime_file]) args.push("--ro-bind", path, path);
  args.push("--perms", "0000", "--tmpfs", layout.roots.private_directory, "--remount-ro", layout.roots.private_directory,
    "--ro-bind", deviceMask ? "/dev/null" : layout.empty_mask, layout.roots.private_file,
    "--bind", layout.scratch, layout.scratch, "--chdir", layout.scratch,
    "--", "/usr/bin/python3", "-I", "-S", "-c", actor, JSON.stringify(layout));
  return args;
}
function verify(observed: any, write: boolean, anchored: boolean, deviceMask: boolean): void {
  assert.equal(observed.started, true, "a failed bootstrap cannot prove a mount denial");
  assert.equal(observed.renames.length, 8);
  for (const entry of observed.renames) {
    assert.equal(entry.result.ok, write && !anchored, `ancestor binding: ${JSON.stringify(entry)}`);
    if (write && anchored) assert.equal(entry.result.errno, 16, "protected ancestors must be busy mountpoints");
  }
  for (const entry of observed.siblings) assert.equal(entry.ok, write, "ancestor anchors must preserve sibling writes");
  for (const entry of observed.immutable_writes) assert.deepEqual(entry, { ok: false, errno: 30 }, "immutable data must be on read-only mounts");
  assert.equal(observed.private_directory_read, "denied");
  assert.equal(observed.private_file_read, deviceMask ? "empty" : "denied");
  assert.equal(observed.private_writes[0].ok, false);
  assert.equal(observed.private_writes[1].ok, deviceMask, "a device mask is not a read-only regular file");
  assert.equal(observed.symlink_private_read, "denied");
  assert.equal(observed.workspace_write.ok, write);
  assert.equal(observed.public_rename.ok, write, "ordinary public renames must remain usable");
  assert.equal(observed.scratch_write.ok, true);
}
function selfTest(): void {
  const layout = { project: "/project", roots: { a: "/project/first/deep/private", b: "/project/second/file" } } as Layout;
  assert.deepEqual(anchors(layout), ["/project/first", "/project/second", "/project/first/deep"]);
  assert.equal(beneath("/project-other/file", "/project"), false);
  const syntax = spawnSync("/usr/bin/python3", ["-I", "-S", "-c", "import ast, sys; ast.parse(sys.stdin.read())"],
    { input: actor, encoding: "utf8", timeout: 10_000, env: { PATH: "/usr/bin:/bin" } });
  assert.equal(syntax.status, 0, syntax.stderr);
}

const { values } = parseArgs({ options: { report: { type: "string" }, "self-test": { type: "boolean" } } });
selfTest();
if (!values["self-test"]) {
  assert.equal(process.platform, "linux", "native mount receipts require Linux");
  assert(values.report, "--report is required for native receipts");
  const bwrap = spawnSync("/usr/bin/bwrap", ["--version"],
    { encoding: "utf8", timeout: 10_000, maxBuffer: 4096, env: { PATH: "/usr/bin:/bin" } });
  assert.equal(bwrap.status, 0, "bubblewrap version query failed");
  const python = spawnSync("/usr/bin/python3", ["-I", "-S", "--version"],
    { encoding: "utf8", timeout: 10_000, maxBuffer: 4096, env: { PATH: "/usr/bin:/bin" } });
  assert.equal(python.status, 0, "Python version query failed");
  const rows = [];
  let outcome = "failed";
  try {
    for (const [write, anchored, deviceMask] of [[true, false, false], [true, true, false],
      [false, false, false], [false, true, false], [true, true, true]]) {
      const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-linux-mounts-")));
      try {
        const layout = fixture(root, write);
        symlinkSync(layout.private_data, layout.alias);
        const result = spawnSync("/usr/bin/bwrap", argumentsFor(layout, anchored, deviceMask),
          { encoding: "utf8", timeout: 15_000, maxBuffer: 64 * 1024, env: { PATH: "/usr/bin:/bin" } });
        assert.equal(result.status, 0, `mount bootstrap failed: ${result.error ?? result.stderr}`);
        const packet = JSON.parse(result.stdout);
        assert.equal(packet.jsonrpc, "2.0");
        assert.equal(packet.id, "mount-proof");
        const ownerDataUnchanged = [...layout.immutable_data, layout.private_data, layout.roots.private_file]
          .every(path => readFileSync(path, "utf8") === "owned-fixture-only");
        rows.push({ write, anchored, deviceMask, observations: packet.result, owner_data_unchanged: ownerDataUnchanged });
        verify(packet.result, write, anchored, deviceMask);
        assert.equal(ownerDataUnchanged, true, "owner data changed");
      } finally { rmSync(root, { recursive: true, force: true }); }
    }
    outcome = "passed";
  } finally {
    const sources = ["scripts/probes/plugin-native-entry/linux_mounts.mts", ".github/workflows/plugin-linux-mounts.yml"];
    const hash = createHash("sha256");
    for (const path of sources) hash.update(path + "\0").update(readFileSync(path));
    const report = { platform: process.platform, architecture: process.arch, kernel_release: release(),
      bubblewrap_version: bwrap.stdout.trim(), python_version: python.stdout.trim(), checkout: process.env.GITHUB_SHA ?? null,
      run_id: process.env.GITHUB_RUN_ID ?? null, outcome, sources, source_sha256: hash.digest("hex"), rows,
      scope: "Mount primitive experiment only. No Landlock, native helper/runtime admission, network capability, Windows or production activation proof." };
    mkdirSync(dirname(resolve(values.report)), { recursive: true });
    writeFileSync(values.report, JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
  }
  console.log(`Verified ${rows.length} mount cases with positive startup, public writes/renames and owner-data checks`);
}
