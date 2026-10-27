/** Native macOS/Linux proof, not production enforcement or full acceptance.
 * node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --python /path/to/python3
 */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { closeSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { release, tmpdir } from "node:os";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

type Runtime = { kind: "python" | "node"; executable: string; prefix: string; include: string; libdir: string; library?: string; version: string };
type Artifacts = { executable: string; addon?: string; landlock?: string };
type Flags = { read: boolean; write: boolean; shell: boolean; unrestricted?: boolean };
type Observation = Record<string, boolean | null | boolean[]>;
const source = dirname(fileURLToPath(import.meta.url));
const environment = { PATH: "/usr/bin:/bin:/usr/sbin:/sbin", PYTHONNOUSERSITE: "1" };
const isLinux = process.platform === "linux";

function command(program: string, args: string[], options: Record<string, unknown> = {}): string {
  const result = spawnSync(program, args, { encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024, env: environment, ...options });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${program} failed (${result.signal ?? result.status}): ${result.stderr}`);
  return result.stdout;
}

function pythonInfo(program: string): Runtime {
  const query = 'import json,sys,sysconfig; print(json.dumps({"executable":sys.executable,"prefix":sys.base_prefix,"include":sysconfig.get_path("include"),"libdir":sysconfig.get_config_var("LIBDIR"),"version":sys.version.split()[0]}))';
  const info: Runtime = JSON.parse(command(realpathSync(program), ["-I", "-S", "-c", query]));
  const [major, minor] = info.version.split(".");
  const library = join(info.libdir, `libpython${major}.${minor}.${isLinux ? "so" : "dylib"}`);
  assert(existsSync(library), `an embeddable dynamic Python library is required: ${library}`);
  return { ...info, kind: "python", library: realpathSync(library) };
}

function nodeInfo(): Runtime {
  const executable = realpathSync(process.execPath);
  const prefix = dirname(dirname(executable));
  const include = join(prefix, "include", "node");
  assert(existsSync(join(include, "node_api.h")), "Node development headers are required");
  return { kind: "node", executable, prefix, include, libdir: join(prefix, "lib"), version: process.versions.node };
}

function build(root: string, runtime: Runtime): Artifacts {
  const clang = isLinux ? "/usr/bin/cc" : command("/usr/bin/xcrun", ["--find", "clang"]).trim();
  const platform = isLinux ? [] : ["-isysroot", command("/usr/bin/xcrun", ["--show-sdk-path"]).trim()];
  const warnings = ["-O1", "-Wall", "-Wextra", "-Werror", ...isLinux ? [] : ["-Wno-error=deprecated-declarations"]];
  const [major, minor] = runtime.version.split(".");
  const executable = join(root, "native-proof");
  const landlock = isLinux ? join(root, "landlock-proof") : undefined;
  if (landlock) command(clang, [...warnings, join(source, "linux_exec.c"), "-o", landlock]);
  if (runtime.kind === "node") {
    const addon = join(root, "native.node");
    command(clang, [...platform, ...warnings, join(source, "native_exec.c"), "-o", executable]);
    command(clang, [...platform, ...warnings, ...isLinux ? ["-shared", "-fPIC"] : ["-bundle", "-undefined", "dynamic_lookup"],
      `-I${runtime.include}`, join(source, "macos_node.c"), "-o", addon]);
    return { executable, addon, landlock };
  }
  command(clang, [...platform, ...warnings,
    `-I${runtime.include}`, join(source, "macos_python.c"), join(runtime.libdir, `libpython${major}.${minor}.${isLinux ? "so" : "dylib"}`),
    `-Wl,-rpath,${runtime.libdir}`, "-o", executable]);
  return { executable, landlock };
}

function policy(runtime: Runtime, paths: Record<string, string>, flags: Flags): string {
  const allow = (operation: string, path: string) => `(allow ${operation} (subpath ${JSON.stringify(path)}))`;
  // The child runtime requires directory-data access to the root object itself.
  // A literal root rule does not grant reads of descendant files/directories.
  const lines = ["(version 1)", "(deny default)", "(allow file-read-metadata)", "(allow sysctl-read)",
    "(allow signal (target same-sandbox))", "(allow process-info* (target same-sandbox))",
    '(allow file-read* file-write* (literal "/dev/null"))',
    '(allow file-read-data (literal "/"))'];
  for (const path of ["/System", "/usr/lib", "/bin", "/usr/bin", runtime.libdir, dirname(runtime.executable), paths.package, paths.scratch]) {
    lines.push(allow("file-read* file-map-executable", path));
  }
  // Framework Python's libpython symlink points outside LIBDIR to the
  // framework binary. Admit that exact trusted file for descendant startup.
  if (runtime.library) lines.push(`(allow file-read* file-map-executable (literal ${JSON.stringify(runtime.library)}))`);
  lines.push(allow("file-write*", paths.scratch));
  if (flags.read) lines.push(allow("file-read*", paths.project));
  if (flags.write) lines.push(allow("file-write*", paths.project));
  for (const path of [paths.private, paths.protected]) {
    lines.push(`(deny file-read* file-write* (subpath ${JSON.stringify(path)}))`);
  }
  lines.push(`(deny file-write-unlink (literal ${JSON.stringify(paths.project)}))`);
  if (flags.shell) lines.push("(allow process-fork)", "(allow process-exec)");
  return lines.join("\n");
}

function fixture(root: string): Record<string, string> {
  const paths = Object.fromEntries(["package", "project", "private", "scratch"].map(name => [name, join(root, name)]));
  for (const path of Object.values(paths)) mkdirSync(path, { mode: 0o700 });
  paths.protected = join(paths.project, ".owner-private");
  mkdirSync(paths.protected, { mode: 0o700 });
  writeFileSync(join(paths.protected, "data"), "protected temporary fixture", { mode: 0o600 });
  for (const file of ["fixture.py", "fixture.cjs"]) copyFileSync(join(source, file), join(paths.package, file));
  for (const path of [join(paths.package, "data"), join(paths.project, "data"), join(paths.private, "data")]) {
    writeFileSync(path, "owned temporary fixture", { mode: 0o600 });
  }
  symlinkSync(join(paths.private, "data"), join(paths.package, "escape"));
  return paths;
}

function linuxLaunch(artifacts: Artifacts, runtime: Runtime, paths: Record<string, string>, profile: string, flags: Flags, args: string[]) {
  const grants: string[] = [];
  const grant = (right: "R" | "W", path: string) => {
    assert(path.startsWith("/") && path !== "/" && !/[\t\n\r\0]/.test(path), "bounded absolute native proof root required");
    if (existsSync(path)) grants.push(`${right}\t${path}`);
  };
  for (const path of ["/usr", "/lib", "/lib64", "/bin", "/etc/ld.so.cache", "/etc/localtime",
    "/dev/null", "/dev/urandom", "/dev/random", runtime.prefix, paths.package, paths.scratch,
    artifacts.executable, artifacts.addon, join(source, "node_bootstrap.cjs"), profile]) {
    if (path) grant("R", path);
  }
  grant("W", "/dev/null");
  grant("W", paths.scratch);
  if (flags.read) grant("R", paths.project);
  if (flags.write) grant("W", paths.project);
  const policyPath = profile + ".landlock";
  writeFileSync(policyPath, grants.join("\n") + "\n", { mode: 0o600 });
  const bubblewrap = ["--die-with-parent", "--unshare-user", "--unshare-pid", "--unshare-net",
    "--unshare-ipc", "--unshare-uts", "--disable-userns", "--cap-drop", "ALL", "--ro-bind", "/", "/",
    "--proc", "/proc", "--dev", "/dev", "--bind", paths.scratch, paths.scratch];
  if (flags.write) bubblewrap.push("--bind", paths.project, paths.project);
  // Mount mode-000 empty directories over protected state even when the
  // workspace ancestor is writable/readable. Landlock grants are additive.
  for (const privatePath of [paths.private, paths.protected]) {
    bubblewrap.push("--perms", "000", "--tmpfs", privatePath, "--remount-ro", privatePath);
  }
  bubblewrap.push("--chdir", paths.cwd, "--", artifacts.landlock!, policyPath, artifacts.executable, ...args);
  return { program: "/usr/bin/bwrap", args: bubblewrap };
}

function observe(artifacts: Artifacts, runtime: Runtime, paths: Record<string, string>, profile: string, mode: string, flags: Flags): Observation {
  const workerAddon = join(paths.package, "native.node");
  if (artifacts.addon && mode === "inspect") copyFileSync(artifacts.addon, workerAddon);
  const identity = statSync(join(paths.private, "data"), { bigint: true });
  const input = { platform: process.platform, runtime: runtime.executable, addon: workerAddon, private_dev: identity.dev.toString(), private_ino: identity.ino.toString(), python: runtime.executable, mode, confined: profile !== "-", package_file: join(paths.package, "data"),
    project_file: join(paths.project, "data"), private_file: join(paths.private, "data"),
    scratch_file: join(paths.scratch, "roundtrip"), project_write: join(paths.project, "written"),
    symlink_private: join(paths.package, "escape"), protected_file: join(paths.protected, "data"),
    scratch: paths.scratch, project: paths.project, exec_marker: join(paths.scratch, "exec-receipt.json") };
  const inherited = openSync(input.private_file, "r");
  try {
    const args = runtime.kind === "python"
      ? [runtime.executable, runtime.prefix, profile, join(paths.package, "fixture.py"), JSON.stringify(input)]
      : [runtime.executable, join(source, "node_bootstrap.cjs"), artifacts.addon!, profile, join(paths.package, "fixture.cjs"), JSON.stringify(input)];
    const launch = isLinux && !flags.unrestricted ? linuxLaunch(artifacts, runtime, paths, profile, flags, args) : { program: artifacts.executable, args };
    const stdout = command(launch.program, launch.args,
      { cwd: paths.cwd, stdio: ["ignore", "pipe", "pipe", "ignore", "ignore", "ignore", "ignore", "ignore", "ignore", inherited] });
    if (runtime.kind === "node" && mode === "exec" && stdout === "") {
      return JSON.parse(readFileSync(input.exec_marker, "utf8"));
    }
    return JSON.parse(stdout);
  } finally { closeSync(inherited); }
}

function expected(flags: Flags): Observation {
  const unrestricted = !!flags.unrestricted;
  return { package_read: true, scratch_write: true, scratch_read: true, project_read: flags.read,
    private_read: unrestricted, private_list: unrestricted, private_write: unrestricted,
    project_list: flags.read, cannot_relax: unrestricted ? null : true,
    protected_read: unrestricted, protected_write: unrestricted,
    hardlink_private: unrestricted, project_rename: unrestricted,
    symlink_private_read: unrestricted, project_write: flags.write,
    inherited_descriptor: false, thread: true, shell: flags.shell, native_fork: flags.shell,
    thread_reads: [flags.read, unrestricted],
    thread_shell: flags.shell,
    native_child_reads: flags.shell ? [flags.read, unrestricted] : null,
    child_reads: flags.shell ? [flags.read, unrestricted] : null,
    written_project_read: flags.write && flags.read };
}

function check(root: string, artifacts: Artifacts, runtime: Runtime, name: string, flags: Flags) {
  const directory = join(root, name);
  mkdirSync(directory, { mode: 0o700 });
  const paths = fixture(directory);
  paths.cwd = flags.read ? paths.project : paths.scratch;
  const profile = flags.unrestricted ? "-" : join(directory, "policy.sb");
  if (!flags.unrestricted) writeFileSync(profile, isLinux ? `shell=${flags.shell ? 1 : 0}\n` : policy(runtime, paths, flags), { mode: 0o600 });
  const actual = observe(artifacts, runtime, paths, profile, "inspect", flags);
  assert.deepEqual(actual, expected(flags), `${name}: capability boundary differs`);
  const execution = observe(artifacts, runtime, paths, profile, "exec", flags);
  assert.deepEqual(execution, { self_exec: flags.shell }, `${name}: interpreter exec boundary differs`);
  return { name, flags, observations: actual, self_exec: execution.self_exec };
}

function main() {
  assert(["darwin", "linux"].includes(process.platform), "native proof requires macOS or Linux; other platforms are unproved");
  const { values } = parseArgs({ options: { runtime: { type: "string", default: "python" }, python: { type: "string" }, report: { type: "string" } } });
  assert(["python", "node"].includes(values.runtime), "runtime must be python or node");
  if (values.runtime === "python") assert(values.python, "supply an explicit trusted Python with development headers and dynamic library");
  const runtime = values.runtime === "node" ? nodeInfo() : pythonInfo(values.python!);
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-plugin-native-proof-")));
  try {
    const artifacts = build(root, runtime);
    const rows = [
      check(root, artifacts, runtime, "unconfined-control", { read: true, write: true, shell: true, unrestricted: true }),
      check(root, artifacts, runtime, "no-capabilities", { read: false, write: false, shell: false }),
      check(root, artifacts, runtime, "read-only", { read: true, write: false, shell: false }),
      check(root, artifacts, runtime, "write-without-read", { read: false, write: true, shell: false }),
      check(root, artifacts, runtime, "shell-without-read", { read: false, write: false, shell: true }),
      check(root, artifacts, runtime, "read-write-without-shell", { read: true, write: true, shell: false }),
      check(root, artifacts, runtime, "read-shell-without-write", { read: true, write: false, shell: true }),
      check(root, artifacts, runtime, "write-shell-without-read", { read: false, write: true, shell: true }),
      check(root, artifacts, runtime, "declared-read-write-shell", { read: true, write: true, shell: true }),
    ];
    const digest = createHash("sha256");
    const sources = ["run.mts", "macos_python.c", "macos_node.c", "native_exec.c", "node_bootstrap.cjs", "fixture.py", "fixture.cjs", "process_policy.h", "linux_filter.h", "linux_exec.c"];
    for (const file of sources) {
      digest.update(file + "\0").update(readFileSync(join(source, file)));
    }
    const report = { recorded_at: new Date().toISOString(), kernel_release: release(), sources, source_sha256: digest.digest("hex"), platform: process.platform, architecture: process.arch, runtime: runtime.kind, version: runtime.version,
      mechanism: isLinux ? "bubblewrap private mounts + Landlock before runtime startup + synchronized seccomp before fixture execution" : "trusted native/runtime bootstrap followed by native sandbox_init before fixture execution",
      limitation: "experimental native mechanisms; macOS uses deprecated Seatbelt API; not production packaging or Windows acceptance", rows };
    if (values.report) writeFileSync(values.report, JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
    console.log(JSON.stringify(report, null, 2));
  } finally { rmSync(root, { recursive: true, force: true }); }
}

main();
