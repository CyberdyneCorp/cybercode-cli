/**
 * Selection mentions (`editor-integration` → Selection mentions) and the editor context
 * part (`editor-integration` → Editor context attachment).
 */
import { isAbsolute, relative, sep } from "node:path";

export interface LineSelection {
  /** 1-based, inclusive. */
  start: number;
  end: number;
}

/** `file`'s path relative to `location`, with `/` separators, or the absolute path outside it. */
export function locationPath(location: string, file: string): string {
  const rel = relative(location, file);
  if (rel === "" || rel.startsWith("..") || isAbsolute(rel)) return file;
  return rel.split(sep).join("/");
}

/** `@path#L12-30`, or `@path#L12` for one line. */
export function mention(location: string, file: string, lines: LineSelection): string {
  const path = locationPath(location, file);
  const range = lines.start === lines.end ? `${lines.start}` : `${lines.start}-${lines.end}`;
  return `@${path}#L${range}`;
}

/**
 * VS Code selections are 0-based and a selection ending at column 0 of a line does not
 * include it; the result is 1-based and inclusive, `undefined` for an empty selection.
 */
export function selectedLines(start: { line: number; character: number }, end: { line: number; character: number }): LineSelection | undefined {
  if (start.line === end.line && start.character === end.character) return undefined;
  const last = end.character === 0 && end.line > start.line ? end.line - 1 : end.line;
  return { start: start.line + 1, end: last + 1 };
}

/** The `file` prompt part for the active editor. */
export function filePart(location: string, file: string, lines: LineSelection | undefined, withRange: boolean): { type: "file"; path: string; range?: { start_line: number; end_line: number } } {
  const path = locationPath(location, file);
  if (!lines || !withRange) return { type: "file", path };
  return { type: "file", path, range: { start_line: lines.start, end_line: lines.end } };
}
