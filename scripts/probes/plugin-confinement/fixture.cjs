// Untrusted fixture operations. Native methods enter the kernel directly.
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { Worker } = require("node:worker_threads");
const { native, input: c } = globalThis.nativeProof;

function runChild(program, args, options) {
  if (c.kind !== "deno") return spawnSync(program, args, options);
  // Deno's Node-compatible spawnSync returned {} for an OS-denied spawn.
  // Its native API reports an explicit typed error and supports positive controls.
  try {
    const result = new Deno.Command(program, { args, env: options.env, clearEnv: true,
      stdout: "piped", stderr: "piped" }).outputSync();
    const decode = new TextDecoder();
    return { status: result.code, signal: result.signal, stdout: decode.decode(result.stdout), stderr: decode.decode(result.stderr) };
  } catch (error) {
    if (!(error instanceof Deno.errors.PermissionDenied)) throw error;
    return { error, status: null };
  }
}

function canList(directory) {
  try { fs.readdirSync(directory); return true; }
  catch { return false; }
}

function inheritedDescriptor() {
  try {
    const actual = fs.fstatSync(9, { bigint: true });
    return actual.dev.toString() === c.private_dev && actual.ino.toString() === c.private_ino;
  } catch { return false; }
}

function cannotRelax() {
  if (!c.confined) return null;
  return !native.relax() && !native.read(c.private_file);
}

function hardlinkPrivate() {
  const alias = path.join(c.scratch, "private-alias");
  try { fs.linkSync(c.private_file, alias); return native.read(alias); }
  catch { return false; }
}

function renameProject() {
  const target = path.join(c.scratch, "moved-project");
  try { fs.renameSync(c.project, target); }
  catch { return false; }
  fs.renameSync(target, c.project);
  return true;
}

function shell() {
  const result = runChild("/bin/sh", ["-c", "exit 0"], { timeout: 5000, env: {} });
  return !result.error && result.status === 0;
}

function childReads() {
  const script = c.kind === "deno"
    ? `import fs from 'node:fs';const r=p=>{try{fs.readFileSync(p);return true}catch{return false}};console.log(JSON.stringify(Deno.args.map(r)));`
    : `const fs=require('node:fs');const r=p=>{try{fs.readFileSync(p);return true}catch{return false}};process.stdout.write(JSON.stringify(process.argv.slice(1).map(r)));`;
  const args = c.kind === "deno" ? ["eval", "--no-config", "--no-lock", script] : ["-e", script];
  const result = runChild(c.runtime, [...args, c.project_file, c.private_file], {
    encoding: "utf8", timeout: 5000, env: { PATH: "/usr/bin:/bin", DENO_DIR: path.join(c.scratch, "deno-cache") },
  });
  if (result.error) return null;
  if (result.status !== 0) throw new Error(`declared ${c.kind} child failed: ${result.status}/${result.signal}: ${result.stderr}`);
  return JSON.parse(result.stdout);
}

function nativeChildReads() {
  const results = [c.project_file, c.private_file].map(file => runChild("/bin/cat", [file], {
    timeout: 5000, env: {}, encoding: "utf8",
  }));
  if (results.every(result => result.error)) return null;
  return results.map(result => !result.error && result.status === 0);
}

async function thread() {
  return new Promise((resolve, reject) => {
    const worker = new Worker(`
      const { parentPort, workerData: c } = require('node:worker_threads');
      const native = require(c.addon);
      let spawned = false;
      if (c.kind === 'deno') {
        try { spawned = new Deno.Command('/bin/sh', { args: ['-c', 'exit 0'], env: {}, clearEnv: true,
          stdout: 'null', stderr: 'null' }).outputSync().success; }
        catch (error) { if (!(error instanceof Deno.errors.PermissionDenied)) throw error; }
      } else {
        const child = require('node:child_process').spawnSync('/bin/sh', ['-c', 'exit 0'], { timeout: 5000, env: {} });
        spawned = !child.error && child.status === 0;
      }
      parentPort.postMessage({ reads: [native.read(c.project_file), native.read(c.private_file)],
        shell: spawned });
    `, { eval: true, workerData: c });
    const timer = setTimeout(() => { worker.terminate(); reject(new Error("worker proof timed out")); }, 5000);
    let reads = null;
    worker.once("message", message => { reads = message; });
    worker.once("error", error => { clearTimeout(timer); reject(error); });
    worker.once("exit", code => { clearTimeout(timer); resolve({ thread: code === 0 && reads !== null,
      thread_reads: reads?.reads ?? null, thread_shell: reads?.shell ?? null }); });
  });
}

async function inspect() {
  const observations = {
    package_read: native.read(c.package_file),
    startup_artifacts: process.env.CYBER_PROOF_AUTO_PRELOAD !== undefined || process.env.CYBER_PROOF_AUTO_DOTENV !== undefined,
    scratch_write: native.write(c.scratch_file),
    scratch_read: native.read(c.scratch_file),
    project_read: native.read(c.project_file),
    project_list: canList(path.dirname(c.project_file)),
    private_list: canList(path.dirname(c.private_file)),
    private_write: native.write(c.private_file),
    protected_read: native.read(c.protected_file),
    protected_write: native.write(c.protected_file),
    hardlink_private: hardlinkPrivate(),
    cannot_relax: cannotRelax(),
    private_read: native.read(c.private_file),
    symlink_private_read: native.read(c.symlink_private),
    project_write: native.write(c.project_write),
    inherited_descriptor: inheritedDescriptor(),
    ...await thread(),
    shell: shell(),
    native_fork: native.fork(),
    child_reads: childReads(),
    native_child_reads: nativeChildReads(),
    written_project_read: native.read(c.project_write),
  };
  observations.project_rename = renameProject();
  if (c.initial_exec) observations.filter_replacement_denied = native.replacementDenied();
  process.stdout.write(JSON.stringify(observations));
}

if (c.mode === "exec") {
  process.stdout.write(JSON.stringify({ self_exec: native.selfExec(c.runtime, c.exec_marker, c.kind) }));
} else {
  inspect().catch(error => { console.error(error); process.exitCode = 1; });
}
