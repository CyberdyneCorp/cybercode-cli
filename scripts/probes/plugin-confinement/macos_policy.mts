/** Shared experimental macOS profile; not production host admission. */
import { dirname } from "node:path";

type Runtime = { kind: string; executable: string; libdir: string; library?: string };
type Flags = { read: boolean; write: boolean; shell: boolean };

export function policy(runtime: Runtime, paths: Record<string, string>, flags: Flags, stagedPlatform = false, signingIdentifier?: string, privateUnix = false): string {
  const allow = (operation: string, path: string) => `(allow ${operation} (subpath ${JSON.stringify(path)}))`;
  // The child runtime requires directory-data access to the root object itself.
  // A literal root rule does not grant reads of descendant files/directories.
  const lines = ["(version 1)", "(deny default)", "(allow file-read-metadata)", "(allow sysctl-read)",
    "(allow signal (target same-sandbox))", "(allow process-info* (target same-sandbox))",
    '(allow file-read* file-write* (literal "/dev/null"))',
    '(allow file-read-data (literal "/"))'];
  for (const path of ["/System", "/usr/lib", "/bin", "/usr/bin", runtime.libdir, ...runtime.kind === "native" ? [] : [dirname(runtime.executable)], paths.package, paths.scratch]) {
    lines.push(allow("file-read* file-map-executable", path));
  }
  // Framework Python's libpython symlink points outside LIBDIR to the
  // framework binary. Admit that exact trusted file for descendant startup.
  if (runtime.library) lines.push(`(allow file-read* file-map-executable (literal ${JSON.stringify(runtime.library)}))`);
  if (runtime.kind === "python" && signingIdentifier) {
    // Older framework Python initializes its hash seed from this public device.
    // Do not admit /dev generally or substitute a predictable random seed.
    lines.push('(allow file-read-data (literal "/dev/urandom"))');
  }
  if (runtime.kind === "native") lines.push(`(allow file-read* file-map-executable (literal ${JSON.stringify(runtime.executable)}))`);
  lines.push(allow("file-write*", paths.scratch));
  if (privateUnix) {
    lines.push(`(allow network-inbound (local unix-socket (subpath ${JSON.stringify(paths.scratch)})))`);
    lines.push(`(allow network-outbound (remote unix-socket (subpath ${JSON.stringify(paths.scratch)})))`);
  }
  if (flags.read) lines.push(allow("file-read*", paths.project));
  if (flags.write) lines.push(allow("file-write*", paths.project));
  for (const path of [paths.private, paths.protected]) {
    lines.push(`(deny file-read* file-write* (subpath ${JSON.stringify(path)}))`);
  }
  lines.push(`(deny file-write-unlink (literal ${JSON.stringify(paths.project)}))`);
  if (flags.shell) lines.push("(allow process-fork)", "(allow process-exec)");
  else if (signingIdentifier) lines.push(`(allow process-exec (require-all (signing-identifier ${JSON.stringify(signingIdentifier)}) (literal ${JSON.stringify(runtime.executable)})))`);
  else if (stagedPlatform) lines.push(`(allow process-exec (require-all (process-attribute is-platform-binary) (literal ${JSON.stringify(runtime.executable)})))`);
  return lines.join("\n");
}
