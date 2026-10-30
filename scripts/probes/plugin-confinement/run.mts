/** Native macOS/Linux proof, not production enforcement or full acceptance.
 * node --experimental-strip-types scripts/probes/plugin-confinement/run.mts --python /path/to/python3
 */
import assert from "node:assert/strict";
import { stageNative } from "./macos_stage.mts";
import { policy } from "./macos_policy.mts";
import { spawnSync } from "node:child_process";
import { closeSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { release, tmpdir } from "node:os";
import { createHash, randomUUID } from "node:crypto";
import { basename, dirname, isAbsolute, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

type Runtime = { kind: "python" | "node" | "bun" | "deno" | "native"; executable: string; prefix: string; include: string; libdir: string; library?: string; version: string };
type Artifacts = { executable: string; addon?: string; landlock?: string; initialExec: boolean; stagedPlatform?: boolean; signingIdentifier?: string; trace?: string; packaged?: boolean; unixScratch?: boolean; privateUnix?: boolean };
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

function pythonInfo(program: string, embedded = true): Runtime {
  const query = [
    "import json,sys,sysconfig",
    'info={"executable":sys.executable,"prefix":sys.base_prefix,"include":sysconfig.get_path("include"),"libdir":sysconfig.get_config_var("LIBDIR"),"framework":sysconfig.get_config_var("PYTHONFRAMEWORK"),"version":sys.version.split()[0]}',
    ...embedded || isLinux ? [] : [
      // Framework command-line shims may spawn a second interpreter image.
      // Resolve the loaded image instead of granting that shim another exec.
      "import ctypes,os",
      "size=ctypes.c_uint32(65536)",
      "buffer=ctypes.create_string_buffer(size.value)",
      "image=ctypes.CDLL(None)._NSGetExecutablePath",
      "image.argtypes=[ctypes.c_void_p,ctypes.POINTER(ctypes.c_uint32)]",
      "image.restype=ctypes.c_int",
      "assert image(buffer,ctypes.byref(size))==0, 'runtime executable path exceeds proof bound'",
      'info["executable"]=os.fsdecode(buffer.value)',
    ],
    "print(json.dumps(info))",
  ].join("\n");
  const info: Runtime & { framework?: string } = JSON.parse(command(realpathSync(program), ["-I", "-S", "-c", query]));
  const [major, minor] = info.version.split(".");
  const library = join(info.libdir, `libpython${major}.${minor}.${isLinux ? "so" : "dylib"}`);
  const hasLibrary = existsSync(library);
  assert(!embedded || hasLibrary, `an embeddable dynamic Python library is required: ${library}`);
  const framework = !embedded && info.framework ? join(info.prefix, info.framework) : undefined;
  const resolvedLibrary = hasLibrary ? realpathSync(library)
    : framework && existsSync(framework) ? realpathSync(framework) : undefined;
  // Relocatable framework builds can retain a build-machine LIBDIR. Direct
  // execution needs the actual runtime's stdlib and exact framework image, not
  // an embedding library or a grant to that stale build-tree directory.
  return { ...info, kind: "python", library: resolvedLibrary,
    libdir: embedded ? info.libdir : realpathSync(join(info.prefix, "lib")) };
}

function nodeInfo(): Runtime {
  const executable = realpathSync(process.execPath);
  const prefix = dirname(dirname(executable));
  const include = join(prefix, "include", "node");
  assert(existsSync(join(include, "node_api.h")), "Node development headers are required");
  return { kind: "node", executable, prefix, include, libdir: join(prefix, "lib"), version: process.versions.node };
}

function javascriptInfo(kind: "bun" | "deno", program: string, scratch: string): Runtime {
  assert(isAbsolute(program), "supply an absolute trusted runtime executable");
  const executable = realpathSync(program);
  const line = command(executable, ["--version"], { cwd: scratch, env: { ...environment, HOME: scratch, XDG_CONFIG_HOME: scratch } }).trim().split("\n")[0];
  return { kind, executable, prefix: dirname(executable), include: nodeInfo().include,
    libdir: dirname(executable), version: kind === "deno" ? line.split(/\s+/)[1] : line };
}

function startupControl(root: string, runtime: Runtime) {
  if (runtime.kind !== "bun") return undefined;
  const control = join(root, "startup-control");
  mkdirSync(control, { mode: 0o700 });
  const paths = fixture(control);
  const script = 'console.log(JSON.stringify({preload:process.env.CYBER_PROOF_AUTO_PRELOAD==="fixture-only",dotenv:process.env.CYBER_PROOF_AUTO_DOTENV==="fixture-only"}))';
  const observed = JSON.parse(command(runtime.executable, ["-e", script], {
    cwd: paths.project, env: { ...environment, HOME: paths.scratch, XDG_CONFIG_HOME: paths.scratch },
  }));
  assert.deepEqual(observed, { preload: true, dotenv: true }, "unhardened startup control must actually load both project artifacts");
  return observed;
}

function outside(root: string, allowed: string): boolean {
  const child = relative(realpathSync(allowed), root);
  return child === ".." || child.startsWith(".." + sep) || isAbsolute(child);
}

function build(root: string, runtime: Runtime, initialExec: boolean, signingRuntime = false, packagedHelper?: string): Artifacts {
  const clang = isLinux ? "/usr/bin/cc" : command("/usr/bin/xcrun", ["--find", "clang"]).trim();
  const platform = isLinux ? [] : ["-isysroot", command("/usr/bin/xcrun", ["--show-sdk-path"]).trim()];
  const warnings = ["-O1", "-Wall", "-Wextra", "-Werror", ...isLinux ? [] : ["-Wno-error=deprecated-declarations"]];
  const [major, minor] = runtime.version.split(".");
  const executable = join(root, "native-proof");
  const landlock = isLinux ? packagedHelper ? executable : join(root, "landlock-proof") : undefined;
  if (packagedHelper) copyFileSync(packagedHelper, executable);
  if (landlock && !packagedHelper) command(clang, [...warnings, join(source, "linux_exec.c"), "-o", landlock]);
  if (runtime.kind === "native") {
    command(clang, [...platform, ...warnings, "-pthread", join(source, "fixture_native.c"), "-o", runtime.executable]);
    if (!packagedHelper) command(clang, [...platform, ...warnings, join(source, isLinux ? "native_exec.c" : "macos_exec.c"), "-o", executable]);
    return { executable, landlock, initialExec };
  }
  if (runtime.kind !== "python") {
    const addon = join(root, "native.node");
    if (!packagedHelper) command(clang, [...platform, ...warnings, join(source, signingRuntime ? "macos_exec.c" : "native_exec.c"), "-o", executable]);
    command(clang, [...platform, ...warnings, ...isLinux ? ["-shared", "-fPIC"] : ["-bundle", "-undefined", "dynamic_lookup"],
      `-I${runtime.include}`, join(source, "macos_node.c"), "-o", addon]);
    return { executable, addon, landlock, initialExec };
  }
  if (packagedHelper) return { executable, landlock, initialExec };
  if (signingRuntime) {
    command(clang, [...platform, ...warnings, join(source, "macos_exec.c"), "-o", executable]);
    return { executable, initialExec };
  }
  command(clang, [...platform, ...warnings,
    `-I${runtime.include}`, join(source, "macos_python.c"), join(runtime.libdir, `libpython${major}.${minor}.${isLinux ? "so" : "dylib"}`),
    `-Wl,-rpath,${runtime.libdir}`, "-o", executable]);
  return { executable, landlock, initialExec };
}

function fixture(root: string): Record<string, string> {
  const paths = Object.fromEntries(["package", "project", "private", "scratch"].map(name => [name, join(root, name)]));
  for (const path of Object.values(paths)) mkdirSync(path, { mode: 0o700 });
  paths.protected = join(paths.project, ".owner-private");
  mkdirSync(paths.protected, { mode: 0o700 });
  writeFileSync(join(paths.protected, "data"), "protected temporary fixture", { mode: 0o600 });
  writeFileSync(join(paths.project, ".env"), "CYBER_PROOF_AUTO_DOTENV=fixture-only\n", { mode: 0o600 });
  writeFileSync(join(paths.project, "preload.cjs"), 'process.env.CYBER_PROOF_AUTO_PRELOAD="fixture-only";', { mode: 0o600 });
  writeFileSync(join(paths.project, "bunfig.toml"), `preload = [${JSON.stringify(join(paths.project, "preload.cjs"))}]\n`, { mode: 0o600 });
  // Runtime module loaders may open the entrypoint's directory as well as
  // the file. Stage the trusted bootstrap in the already admitted read-only
  // package instead of granting access to the repository containing it.
  for (const file of ["fixture.py", "fixture.cjs", "private_unix.cjs", "node_bootstrap.cjs"]) copyFileSync(join(source, file), join(paths.package, file));
  for (const path of [join(paths.package, "data"), join(paths.project, "data"), join(paths.private, "data")]) {
    writeFileSync(path, "owned temporary fixture", { mode: 0o600 });
  }
  symlinkSync(join(paths.private, "data"), join(paths.package, "escape"));
  return paths;
}

function linuxLaunch(artifacts: Artifacts, runtime: Runtime, paths: Record<string, string>, profile: string, flags: Flags, args: string[]) {
  const grants: string[] = [];
  const grant = (right: "R" | "W" | "U", path: string) => {
    assert(path.startsWith("/") && path !== "/" && !/[\t\n\r\0]/.test(path), "bounded absolute native proof root required");
    if (existsSync(path)) grants.push(`${right}\t${path}`);
  };
  for (const path of ["/usr", "/lib", "/lib64", "/bin", "/etc/ld.so.cache", "/etc/localtime", "/etc/ssl/openssl.cnf",
    "/dev/null", "/dev/urandom", "/dev/random", runtime.prefix, paths.package, paths.scratch,
    artifacts.executable, runtime.executable, profile]) {
    if (path) grant("R", path);
  }
  if (runtime.kind === "bun") {
    // Bun/JSC inspects its own cgroup and mappings during initialization.
    // This procfs is in a mandatory private PID namespace; PID 1 is masked
    // below, and the initial-exec supervisor is non-dumpable. Never substitute
    // host procfs or admit arbitrary filesystem paths through proc aliases.
    for (const path of ["/proc", "/sys/fs/cgroup", "/sys/devices/system/cpu/online",
      "/sys/kernel/mm/transparent_hugepage/enabled"]) grant("R", path);
  }
  grant("W", "/dev/null");
  grant("W", paths.scratch);
  if (artifacts.unixScratch) grant("U", paths.scratch);
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
  if (runtime.kind === "bun") {
    for (const privatePath of ["/proc/1", "/proc/sys"]) {
      bubblewrap.push("--perms", "000", "--tmpfs", privatePath, "--remount-ro", privatePath);
    }
  }
  const direct = artifacts.initialExec && (runtime.kind !== "python" || artifacts.packaged);
  bubblewrap.push("--chdir", paths.cwd, "--", artifacts.landlock!, ...artifacts.packaged ? ["--plugin-native-exec"] : [], policyPath);
  if (artifacts.initialExec) bubblewrap.push(`--initial-shell=${flags.shell ? 1 : 0}`);
  bubblewrap.push(direct ? runtime.executable : artifacts.executable, ...direct ? args.slice(1) : args);
  return { program: "/usr/bin/bwrap", args: bubblewrap };
}

function observe(artifacts: Artifacts, runtime: Runtime, paths: Record<string, string>, profile: string, mode: string, flags: Flags): Observation {
  const workerAddon = join(paths.package, "native.node");
  if (artifacts.addon) copyFileSync(artifacts.addon, workerAddon);
  const identity = statSync(join(paths.private, "data"), { bigint: true });
  const input = { kind: runtime.kind, initial_exec: artifacts.initialExec, private_stream: !!artifacts.privateUnix, platform: process.platform, runtime: runtime.executable, addon: workerAddon, private_dev: identity.dev.toString(), private_ino: identity.ino.toString(), python: runtime.executable, mode, confined: profile !== "-", package_file: join(paths.package, "data"),
    project_file: join(paths.project, "data"), private_file: join(paths.private, "data"),
    scratch_file: join(paths.scratch, "roundtrip"), project_write: join(paths.project, "written"),
    symlink_private: join(paths.package, "escape"), protected_file: join(paths.protected, "data"),
    scratch: paths.scratch, project: paths.project, exec_marker: join(paths.scratch, "exec-receipt.json") };
  const inherited = openSync(input.private_file, "r");
  try {
    const bootstrapProfile = artifacts.initialExec || artifacts.signingIdentifier ? "-" : profile;
    let args = runtime.kind === "python"
      ? [runtime.executable, runtime.prefix, bootstrapProfile, join(paths.package, "fixture.py"), JSON.stringify(input)]
      : runtime.kind === "native"
        ? [...isLinux ? [] : [profile], runtime.executable, mode, input.package_file, input.project_file, input.private_file, input.scratch_file,
          input.project_write, input.symlink_private, input.protected_file, input.scratch, input.project,
          input.private_dev, input.private_ino, input.confined ? "1" : "0"]
        : [runtime.executable, ...runtime.kind === "deno" ? ["run", "-A", "--no-config", "--no-lock", "--node-modules-dir=manual"]
          : runtime.kind === "bun" ? ["--config=/dev/null", "--no-env-file", "--no-install"] : [],
          join(paths.package, "node_bootstrap.cjs"), workerAddon, bootstrapProfile, join(paths.package, "fixture.cjs"), JSON.stringify(input)];
    if (artifacts.signingIdentifier && runtime.kind !== "native" && !artifacts.packaged) {
      // Install policy in the native launcher before any interpreter instruction.
      // Runtime code never installs this candidate's boundary or gets an exec
      // exception; the unchanged runtime has a different signing identity.
      args = runtime.kind === "python"
        ? [profile, runtime.executable, "-I", "-S", join(paths.package, "fixture.py"), JSON.stringify(input)]
        : [profile, ...args];
    }
    if (artifacts.stagedPlatform && runtime.kind === "native" && !flags.unrestricted) {
      // The trusted first launcher closes inherited descriptors, then the system
      // platform executable installs policy before entering the re-signed target.
      args = ["-", "/usr/bin/sandbox-exec", "-f", profile, ...args.slice(1)];
    }
    if (artifacts.packaged) {
      args = runtime.kind === "python"
        ? [runtime.executable, "-I", "-S", join(paths.package, "fixture.py"), JSON.stringify(input)]
        : runtime.kind === "native" && !isLinux ? args.slice(1) : args;
    }
    const launch = artifacts.packaged && flags.unrestricted
      ? { program: runtime.executable, args: args.slice(1) }
      : isLinux && !flags.unrestricted ? linuxLaunch(artifacts, runtime, paths, profile, flags, args)
      : artifacts.packaged ? { program: artifacts.executable, args: ["--plugin-native-exec", profile, ...args] }
      : { program: artifacts.executable, args };
    const options = { cwd: paths.cwd, env: { ...environment, HOME: paths.scratch, XDG_CONFIG_HOME: join(paths.scratch, "config"), DENO_DIR: join(paths.scratch, "deno-cache"),
          XDG_CACHE_HOME: join(paths.scratch, "cache"), BUN_RUNTIME_TRANSPILER_CACHE_PATH: join(paths.scratch, "bun-cache"),
          ...runtime.kind === "native" ? { CYBER_PROOF_PRIVATE: input.private_file, CYBER_PROOF_PRIVATE_STREAM: artifacts.privateUnix ? "1" : "0" } : {} },
        ...mode === "inspect" ? { input: JSON.stringify({ jsonrpc: "2.0", id: "native-proof", method: "tool/execute", params: { name: "native-probe", input: {} } }) + "\n" } : {},
        stdio: [mode === "inspect" ? "pipe" : "ignore", "pipe", "pipe", "ignore", "ignore", "ignore", "ignore", "ignore", "ignore", inherited] };
    let stdout: string;
    try { stdout = command(launch.program, launch.args, options); }
    catch (error) {
      if (isLinux && artifacts.trace && !flags.unrestricted) {
        mkdirSync(artifacts.trace, { recursive: true, mode: 0o700 });
        const filename = join(artifacts.trace, `${basename(dirname(profile))}-${mode}`);
        const traced = spawnSync("/usr/bin/strace", ["-f", "-qq", "-e", "trace=%file,%process,prctl,seccomp", "-o", filename + ".strace", launch.program, ...launch.args],
          { ...options, encoding: "utf8", env: options.env, timeout: 30_000, maxBuffer: 1024 * 1024 } as Parameters<typeof spawnSync>[2]);
        writeFileSync(filename + ".result.txt", `diagnostic retry only; original untraced failure remains fatal\nstatus=${traced.status}; signal=${traced.signal}; error=${traced.error?.message ?? ""}\n${traced.stderr ?? ""}\n${traced.stdout ?? ""}`, { mode: 0o600 });
      }
      throw error;
    }
    if (artifacts.addon && mode === "exec" && stdout === "") {
      return JSON.parse(readFileSync(input.exec_marker, "utf8"));
    }
    const response = JSON.parse(stdout);
    if (mode === "inspect") {
      assert.equal(response.jsonrpc, "2.0", "native host must return a JSON-RPC response");
      assert.equal(response.id, "native-proof", "native host must preserve request identity");
      assert(!("error" in response), "native tool execution must succeed");
      return response.result;
    }
    return response;
  } finally { closeSync(inherited); }
}

function expected(flags: Flags): Observation {
  const unrestricted = !!flags.unrestricted;
  return { stdio_tool: true, package_read: true, scratch_write: true, scratch_read: true, project_read: flags.read,
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
  if (!flags.unrestricted) writeFileSync(profile, isLinux ? `shell=${flags.shell ? 1 : 0}\n` : policy(runtime, paths, flags, artifacts.stagedPlatform, artifacts.signingIdentifier, artifacts.privateUnix), { mode: 0o600 });
  const actual = observe(artifacts, runtime, paths, profile, "inspect", flags);
  const expectations = expected(flags);
  if (artifacts.privateUnix) {
    expectations.private_unix = [true, true];
    assert.deepEqual(actual.private_unix, [true, true],
      `${name}: private UNIX runtime exchange failed: ${JSON.stringify(actual.private_unix)}`);
  }
  // Deno's direct unconfined control reports this descriptor absent too. It
  // cannot establish helper cleanup; the dedicated native FD control does that.
  if (artifacts.packaged && flags.unrestricted) expectations.inherited_descriptor = runtime.kind !== "deno";
  if (artifacts.addon) expectations.startup_artifacts = false;
  if (isLinux && runtime.kind === "bun") {
    expectations.proc_root_reads = [flags.read, !!flags.unrestricted];
    expectations.proc_init_list = !!flags.unrestricted;
  }
  if (artifacts.initialExec) expectations.filter_replacement_denied = !flags.unrestricted;
  if (runtime.kind === "native") {
    expectations.constructor_private_read = !!flags.unrestricted;
    expectations.constructor_fork = flags.shell;
    expectations.native_spawn = flags.shell;
    expectations.package_hardlink = !!flags.unrestricted;
    expectations.package_alias_write = !!flags.unrestricted;
    if (!isLinux) {
      expectations.platform_status_known = true;
      expectations.platform_before = false;
      expectations.platform_after_set = false;
    }
  }
  assert.deepEqual(actual, expectations, `${name}: capability boundary differs`);
  if (runtime.kind === "native") assert.equal(readFileSync(join(paths.package, "data"), "utf8"),
    flags.unrestricted ? "fixture only" : "owned temporary fixture", `${name}: package changed through a writable alias`);
  const execution = observe(artifacts, runtime, paths, profile, "exec", flags);
  assert.deepEqual(execution, { self_exec: flags.shell }, `${name}: interpreter exec boundary differs`);
  const constructorExecution = !isLinux && runtime.kind === "native"
    ? observe(artifacts, runtime, paths, profile, "ctor-exec", flags) : undefined;
  if (constructorExecution) assert.deepEqual(constructorExecution, { self_exec: flags.shell }, `${name}: constructor exec boundary differs`);
  const constructorSpawn = !isLinux && runtime.kind === "native"
    ? observe(artifacts, runtime, paths, profile, "ctor-spawn", flags) : undefined;
  if (constructorSpawn) assert.deepEqual(constructorSpawn, { native_spawn: flags.shell }, `${name}: constructor spawn boundary differs`);
  return { name, flags, observations: actual, self_exec: execution.self_exec,
    ...constructorExecution ? { constructor_self_exec: constructorExecution.self_exec } : {},
    ...constructorSpawn ? { constructor_spawn: constructorSpawn.native_spawn } : {} };
}

function main() {
  assert(["darwin", "linux"].includes(process.platform), "native proof requires macOS or Linux; other platforms are unproved");
  const { values } = parseArgs({ options: { runtime: { type: "string", default: "python" }, "packaged-helper": { type: "string" }, python: { type: "string" }, executable: { type: "string" }, report: { type: "string" }, trace: { type: "string" }, "initial-exec": { type: "boolean", default: false }, "unix-scratch-policy": { type: "boolean", default: false }, "socket-broker": { type: "boolean", default: false }, "broker-private-stream": { type: "boolean", default: false }, "macos-staged-native": { type: "boolean", default: false }, "macos-signing-native": { type: "boolean", default: false }, "macos-signing-runtime": { type: "boolean", default: false }, "macos-private-unix": { type: "boolean", default: false } } });
  assert(["python", "node", "bun", "deno", "native"].includes(values.runtime), "runtime must be python, node, bun, deno or native");
  if (values.runtime === "bun" || values.runtime === "deno") assert(values.executable, "supply an explicit trusted Bun/Deno executable");
  const stagedPlatform = values["macos-staged-native"];
  assert(!stagedPlatform || (!isLinux && values.runtime === "native"), "macos-staged-native requires macOS native runtime");
  const signingNative = values["macos-signing-native"];
  assert(!signingNative || (!isLinux && values.runtime === "native" && !stagedPlatform), "macos-signing-native requires macOS native runtime without staging");
  const signingRuntime = signingNative || values["macos-signing-runtime"];
  assert(!signingRuntime || (!isLinux && !stagedPlatform), "macos-signing-runtime requires macOS without staging");
  assert(!values["macos-private-unix"] || (!isLinux && signingRuntime), "macOS private UNIX proof requires the pre-runtime signing candidate");
  const packagedHelper = values["packaged-helper"];
  if (packagedHelper) {
    assert(isAbsolute(packagedHelper), "packaged helper must be absolute");
    assert(isLinux ? values["initial-exec"] : signingRuntime, "packaged helper requires the pre-runtime mechanism");
  }
  const initialExec = values["initial-exec"] || (values.runtime === "native" && isLinux);
  assert(!initialExec || isLinux, "initial-exec proof currently requires Linux");
  if (values.runtime === "python") assert(values.python, "supply an explicit trusted Python; embedding additionally requires development headers and a dynamic library");
  assert(!values["unix-scratch-policy"] || (isLinux && initialExec && packagedHelper), "unix scratch experiment requires a Linux packaged initial-exec helper");
  assert(!values["socket-broker"] || (isLinux && initialExec && packagedHelper && !values["unix-scratch-policy"]), "socket broker experiment requires a Linux packaged initial-exec helper without the UNIX-9 alternative");
  assert(!values["broker-private-stream"] || values["socket-broker"], "private stream candidate requires the socket-broker reporting mode");
  // Darwin's 104-byte sun_path cannot hold the long per-user TMPDIR plus
  // capability case names. Retain an owned 0700 root under canonical /tmp.
  const temporary = values["macos-private-unix"] ? realpathSync("/tmp") : tmpdir();
  const root = realpathSync(mkdtempSync(join(temporary, "cyber-plugin-native-proof-")));
  try {
    const runtime: Runtime = values.runtime === "node" ? nodeInfo() : values.runtime === "bun" || values.runtime === "deno"
      ? javascriptInfo(values.runtime, values.executable!, root) : values.runtime === "native"
      ? { kind: "native", executable: join(root, "native-fixture"), prefix: "/usr", include: "", libdir: "/usr/lib", version: "fixture-v2" }
      : pythonInfo(values.python!, !signingRuntime && !packagedHelper);
    // A runtime installed straight in /tmp must not accidentally admit all
    // temporary owner fixtures through its directory read grant.
    for (const allowed of [runtime.prefix, runtime.libdir, ...runtime.kind === "native" ? [] : [dirname(runtime.executable)], ...isLinux ? ["/usr", "/lib", "/bin"] : ["/System", "/usr/lib", "/bin", "/usr/bin"]]) {
      if (existsSync(allowed)) assert(outside(root, allowed), `runtime read root overlaps private proof fixtures: ${allowed}`);
    }
    // Literal exec admission must name the same physical executable the kernel
    // resolves, including framework Python's executable symlinks.
    if (signingRuntime && runtime.kind !== "native") runtime.executable = realpathSync(runtime.executable);
    const artifacts: Artifacts = { ...build(root, runtime, initialExec, signingRuntime, packagedHelper), trace: values.trace, packaged: Boolean(packagedHelper), unixScratch: values["unix-scratch-policy"] || values["broker-private-stream"], privateUnix: values["broker-private-stream"] || values["macos-private-unix"] };
    if (signingRuntime) {
      artifacts.signingIdentifier = `com.cyberdyne.probe.bootstrap.${randomUUID()}`;
      command("/usr/bin/codesign", ["--force", "--sign", "-", "--identifier", artifacts.signingIdentifier, "--timestamp=none", artifacts.executable]);
      command("/usr/bin/codesign", ["--verify", "--strict", "--all-architectures", artifacts.executable]);
    }
    const staging = stagedPlatform ? stageNative(runtime.executable, join(root, "staged-native-fixture")) : undefined;
    if (staging) runtime.executable = staging.executable;
    artifacts.stagedPlatform = stagedPlatform;
    const startup_control = startupControl(root, runtime);
    const cases: Array<[string, Flags]> = [
      ["unconfined-control", { read: true, write: true, shell: true, unrestricted: true }],
      ["no-capabilities", { read: false, write: false, shell: false }],
      ["read-only", { read: true, write: false, shell: false }],
      ["write-without-read", { read: false, write: true, shell: false }],
      ["shell-without-read", { read: false, write: false, shell: true }],
      ["read-write-without-shell", { read: true, write: true, shell: false }],
      ["read-shell-without-write", { read: true, write: false, shell: true }],
      ["write-shell-without-read", { read: false, write: true, shell: true }],
      ["declared-read-write-shell", { read: true, write: true, shell: true }],
    ];
    const unsupported_cases = cases.filter(([, flags]) => runtime.kind === "native" && !isLinux && !flags.shell && !stagedPlatform && !signingRuntime)
      .map(([name]) => ({ name, reason: "native macOS shell=false initial-exec mechanism remains unproved; explicit support review required" }));
    const rows = cases.filter(([name]) => !unsupported_cases.some(item => item.name === name))
      .map(([name, flags]) => check(root, artifacts, runtime, name, flags));
    const digest = createHash("sha256");
    const sources = ["run.mts", "macos_policy.mts", "macos_python.c", "macos_node.c", "native_exec.c", "node_bootstrap.cjs", "fixture.py", "fixture.cjs", "private_unix.cjs", "process_policy.h", "linux_filter.h", "linux_exec.c", "linux_initial_exec.h", "fixture_native.c", "macos_exec.c", "macos_stage.mts"];
    for (const file of sources) {
      digest.update(file + "\0").update(readFileSync(join(source, file)));
    }
    const report = { recorded_at: new Date().toISOString(), kernel_release: release(), sources, source_sha256: digest.digest("hex"), platform: process.platform, architecture: process.arch, runtime: runtime.kind, version: runtime.version, startup_control,
      staging, signing_identifier: artifacts.signingIdentifier,
      packaged_helper: packagedHelper ? {
        input_sha256: createHash("sha256").update(readFileSync(packagedHelper)).digest("hex"),
        executed_sha256: createHash("sha256").update(readFileSync(artifacts.executable)).digest("hex"),
        private_copy: true, direct_runtime: true,
      } : undefined,
      socket_broker_candidate: values["socket-broker"],
      private_stream_candidate: values["broker-private-stream"],
      macos_private_unix_candidate: values["macos-private-unix"],
      mechanism: values["socket-broker"] ? "experimental descriptor-pinned socket broker with one-use parent startup-exec budget; notification listener remains live for socket operations; immutable inherited filter" : signingRuntime ? "fresh signing identifier on trusted launcher + exact-target Seatbelt exec rule before any runtime instruction; target signature unchanged; experimental subject-identity candidate" : stagedPlatform ? "ad-hoc-staged native image + system platform launcher Seatbelt policy before first target instruction; platform-subject exact-path exec admission" : initialExec ? "bubblewrap private mounts + Landlock/seccomp before first target instruction + one-shot exec admission; closed listener and immutable filter" : isLinux ? "bubblewrap private mounts + Landlock before runtime startup + synchronized seccomp before fixture execution" : "trusted native/runtime bootstrap followed by native sandbox_init before fixture execution",
      limitation: packagedHelper ? "packaged entry with experimental policy composition; production admission, protected ownership, network/secret integration and Windows acceptance remain required" : "experimental native mechanisms; macOS uses deprecated Seatbelt API; not production packaging or Windows acceptance", unsupported_cases, rows };
    if (values.report) writeFileSync(values.report, JSON.stringify(report, null, 2) + "\n", { mode: 0o600 });
    console.log(JSON.stringify(report, null, 2));
  } finally { rmSync(root, { recursive: true, force: true }); }
}

main();
