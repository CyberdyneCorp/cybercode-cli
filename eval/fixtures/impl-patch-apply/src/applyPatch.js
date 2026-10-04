/**
 * Apply a unified diff to an in-memory set of files. See SPEC.md for the full contract.
 *
 * @param {Record<string, string>} files  path -> content; updated in place on success only
 * @param {string} patchText              the unified diff
 * @param {{ reverse?: boolean, maxOffset?: number }} [options]
 * @returns {{ ok: true, report: { path: string, action: "modify" | "create" | "delete", offsets: number[] }[] }
 *         | { ok: false, error: string }}
 */
export function applyPatch(files, patchText, options = {}) {
  throw new Error("not implemented");
}
