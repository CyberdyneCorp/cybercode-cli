/**
 * The `cyber` executable (`editor-integration` → Extension server connection): the
 * machine-scoped `cyber.executablePath` setting, else `cyber` on `PATH`. Workspace settings
 * never choose it; the caller passes only the user-level value.
 */
import { accessSync, constants, statSync } from "node:fs";
import { delimiter, isAbsolute, join } from "node:path";

export const INSTALL_URL = "https://github.com/CyberdyneCorp/cybercode-cli#install";

export interface ExecutableEnv {
  platform: NodeJS.Platform;
  path: string | undefined;
  pathExt?: string | undefined;
  runnable?: (file: string) => boolean;
}

export function runnable(file: string): boolean {
  try {
    if (!statSync(file).isFile()) return false;
    accessSync(file, process.platform === "win32" ? constants.F_OK : constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/** The executable to run, or `undefined` when none is found. */
export function resolveExecutable(configured: string | undefined, env: ExecutableEnv): string | undefined {
  const usable = env.runnable ?? runnable;
  const setting = configured?.trim();
  if (setting) return isAbsolute(setting) && usable(setting) ? setting : undefined;
  const names = env.platform === "win32" ? (env.pathExt ?? ".EXE;.CMD").split(";").map((ext) => `cyber${ext.toLowerCase()}`) : ["cyber"];
  for (const dir of (env.path ?? "").split(delimiter)) {
    if (!dir || !isAbsolute(dir)) continue;
    const found = names.map((name) => join(dir, name)).find(usable);
    if (found) return found;
  }
  return undefined;
}
