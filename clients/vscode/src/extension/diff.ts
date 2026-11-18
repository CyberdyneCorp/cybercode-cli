/**
 * Unified diffs of pending edit requests (`editor-integration` → Pending edit diff review):
 * parse `metadata.diff` per file and apply its hunks to the on-disk snapshot by exact
 * context match. A diff that does not apply returns `undefined`; the review then shows the
 * raw diff. The server stays the only writer.
 */

export interface DiffLine {
  op: " " | "-" | "+";
  text: string;
  /** Followed by `\ No newline at end of file`. */
  noEol: boolean;
}

export interface Hunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
}

export interface FilePatch {
  /** The path after `a/` (empty for an added file shown as `/dev/null`). */
  oldPath: string;
  newPath: string;
  hunks: Hunk[];
  /** This file's section of the diff, verbatim. */
  raw: string;
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

function stripPrefix(path: string, prefix: string): string {
  const name = path.split("\t")[0] ?? "";
  if (name === "/dev/null") return "";
  return name.startsWith(prefix) ? name.slice(prefix.length) : name;
}

function hunkHeader(line: string): Hunk | undefined {
  const match = HUNK.exec(line);
  if (!match) return undefined;
  const count = (value: string | undefined) => (value === undefined ? 1 : Number(value));
  return { oldStart: Number(match[1]), oldLines: count(match[2]), newStart: Number(match[3]), newLines: count(match[4]), lines: [] };
}

/** Every file section of a unified diff, in order. */
export function parseUnifiedDiff(diff: string): FilePatch[] {
  const files: FilePatch[] = [];
  const lines = diff.split("\n");
  if (lines.at(-1) === "") lines.pop();
  let file: FilePatch | undefined;
  let hunk: Hunk | undefined;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? "";
    if (line.startsWith("--- ") && lines[i + 1]?.startsWith("+++ ")) {
      file = { oldPath: stripPrefix(line.slice(4), "a/"), newPath: stripPrefix((lines[i + 1] ?? "").slice(4), "b/"), hunks: [], raw: "" };
      files.push(file);
      file.raw += `${line}\n${lines[i + 1]}\n`;
      hunk = undefined;
      i++;
      continue;
    }
    if (!file) continue;
    file.raw += `${line}\n`;
    hunk = addLine(file, hunk, line);
  }
  return files;
}

function addLine(file: FilePatch, hunk: Hunk | undefined, line: string): Hunk | undefined {
  const header = hunkHeader(line);
  if (header) {
    file.hunks.push(header);
    return header;
  }
  if (!hunk) return hunk;
  if (line.startsWith("\\")) {
    const last = hunk.lines.at(-1);
    if (last) last.noEol = true;
  } else if (line[0] === " " || line[0] === "-" || line[0] === "+") {
    hunk.lines.push({ op: line[0], text: line.slice(1), noEol: false });
  }
  return hunk;
}

interface Lines {
  lines: string[];
  finalNewline: boolean;
}

function split(text: string): Lines {
  if (text === "") return { lines: [], finalNewline: false };
  const lines = text.split("\n");
  const finalNewline = lines.at(-1) === "";
  if (finalNewline) lines.pop();
  return { lines, finalNewline };
}

/** `original` with `patch`'s hunks applied by exact context, or `undefined` on any mismatch. */
export function applyPatch(original: string, patch: FilePatch): string | undefined {
  const old = split(original);
  const out: string[] = [];
  let cursor = 0;
  let finalNewline = old.finalNewline;
  for (const hunk of patch.hunks) {
    const start = hunk.oldLines === 0 ? hunk.oldStart : hunk.oldStart - 1;
    if (start < cursor || start > old.lines.length) return undefined;
    out.push(...old.lines.slice(cursor, start));
    const applied = applyHunk(old, hunk, start, out);
    if (applied === undefined) return undefined;
    cursor = applied;
    if (cursor === old.lines.length) finalNewline = endsWithNewline(hunk, finalNewline);
  }
  out.push(...old.lines.slice(cursor));
  return out.length === 0 ? "" : out.join("\n") + (finalNewline ? "\n" : "");
}

/** Apply one hunk at `start`, appending to `out`; the old-side index after it. */
function applyHunk(old: Lines, hunk: Hunk, start: number, out: string[]): number | undefined {
  let index = start;
  for (const line of hunk.lines) {
    if (line.op !== "+") {
      if (old.lines[index] !== line.text) return undefined;
      if (line.noEol && (index !== old.lines.length - 1 || old.finalNewline)) return undefined;
      index++;
    }
    if (line.op !== "-") out.push(line.text);
  }
  return index;
}

function endsWithNewline(hunk: Hunk, current: boolean): boolean {
  const last = hunk.lines.filter((line) => line.op !== "-").at(-1);
  if (!last) return current;
  return !last.noEol;
}

/** The patch that undoes `patch` (old and new sides swapped). */
export function reversePatch(patch: FilePatch): FilePatch {
  const flip = { " ": " ", "-": "+", "+": "-" } as const;
  return {
    oldPath: patch.newPath,
    newPath: patch.oldPath,
    raw: patch.raw,
    hunks: patch.hunks.map((hunk) => ({
      oldStart: hunk.newStart,
      oldLines: hunk.newLines,
      newStart: hunk.oldStart,
      newLines: hunk.oldLines,
      lines: hunk.lines.map((line) => ({ ...line, op: flip[line.op] })),
    })),
  };
}
