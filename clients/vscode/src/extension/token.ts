/**
 * The local IDE token file (`editor-integration` → Extension IDE token authentication):
 * `<state>/ide/vscode.token`, written by `cyber ide token create --client vscode`. Its
 * first line is the `cyi_` token and an optional `expires_at=<RFC 3339>` line follows.
 * The value is read only here, at connect time, and kept only in the connection's memory.
 */
import { join } from "node:path";
import { readStateFile } from "./server.js";

const TOKEN = /^cyi_[A-Za-z0-9_-]{43}$/;
const WEEK_MS = 7 * 24 * 60 * 60 * 1000;

export interface TokenFile {
  token: string;
  expiresAt?: number;
}

export function tokenPath(state: string): string {
  return join(state, "ide", "vscode.token");
}

export function parseTokenFile(text: string): TokenFile | undefined {
  const lines = text.split(/\r?\n/).map((line) => line.trim());
  const token = lines[0] ?? "";
  if (!TOKEN.test(token)) return undefined;
  const expires = lines.find((line) => line.startsWith("expires_at="))?.slice("expires_at=".length);
  const expiresAt = expires ? Date.parse(expires) : NaN;
  return Number.isNaN(expiresAt) ? { token } : { token, expiresAt };
}

/** The token file under `state`, or `undefined` when it is missing or malformed. */
export async function readTokenFile(state: string): Promise<TokenFile | undefined> {
  const text = await readStateFile(state, join("ide", "vscode.token"));
  return text === undefined ? undefined : parseTokenFile(text);
}

/** Whether a token expiring at `expiresAt` should be replaced now (within 7 days). */
export function expiresSoon(file: TokenFile, now: number): boolean {
  return file.expiresAt !== undefined && file.expiresAt - now <= WEEK_MS;
}
