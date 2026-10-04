/**
 * Discovery of the locally registered server (Node, Bun and Deno only). Node built-ins are
 * imported lazily so bundling the SDK for a browser does not pull them in.
 */

/** `<state>/server.json`, written by `cyber serve --register`. */
export interface Registration {
  id: string;
  version: string;
  url: string;
  socket?: string | null;
  pid: number;
}

type Env = Record<string, string | undefined>;

function processEnv(): Env {
  return (globalThis as { process?: { env: Env } }).process?.env ?? {};
}

/** The state directory: `$XDG_STATE_HOME/cyber`, else `$CYBER_HOME/state`, else `~/.local/state/cyber`. */
export async function stateDir(env: Env = processEnv()): Promise<string> {
  const { join } = await import("node:path");
  if (env.XDG_STATE_HOME) return join(env.XDG_STATE_HOME, "cyber");
  if (env.CYBER_HOME) return join(env.CYBER_HOME, "state");
  const { homedir } = await import("node:os");
  return join(env.HOME || homedir(), ".local", "state", "cyber");
}

async function readStateFile(name: string, env?: Env): Promise<string | undefined> {
  try {
    const { readFile } = await import("node:fs/promises");
    const { join } = await import("node:path");
    return await readFile(join(await stateDir(env), name), "utf8");
  } catch {
    return undefined;
  }
}

/** The registered server, or `undefined` when none is registered (or not on Node). */
export async function readRegistration(env?: Env): Promise<Registration | undefined> {
  const text = await readStateFile("server.json", env);
  if (text === undefined) return undefined;
  try {
    const reg = JSON.parse(text) as Registration;
    return typeof reg.url === "string" ? reg : undefined;
  } catch {
    return undefined;
  }
}

/** The local server password, or `undefined` when the file is missing or unreadable. */
export async function readPassword(env?: Env): Promise<string | undefined> {
  const text = (await readStateFile("password", env))?.trim();
  return text ? text : undefined;
}
