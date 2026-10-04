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
  const { reverse = false, maxOffset = 100 } = options;
  let sections;
  try {
    sections = parsePatch(patchText);
  } catch (error) {
    if (error instanceof PatchError) return { ok: false, error: error.message };
    throw error;
  }
  if (reverse) sections = sections.map(invertSection);

  const staged = new Map(Object.entries(files));
  const report = [];
  for (const section of sections) {
    const outcome = applySection(staged, section, maxOffset);
    if (outcome.error) return { ok: false, error: outcome.error };
    report.push(outcome);
  }

  for (const path of Object.keys(files)) {
    if (!staged.has(path)) delete files[path];
  }
  for (const [path, content] of staged) files[path] = content;
  return { ok: true, report };
}

class PatchError extends Error {}

const lineError = (lineNo, message) => new PatchError(`line ${lineNo}: ${message}`);

const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;
const DEV_NULL = "/dev/null";

// ---------------------------------------------------------------------------------------------
// Parsing

function parsePatch(text) {
  const lines = text.split("\n");
  if (text.endsWith("\n")) lines.pop();

  const sections = [];
  let section = null;
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.startsWith("--- ")) {
      requireHunks(section);
      section = parseFileHeader(lines, i);
      sections.push(section);
      i += 2;
    } else if (section && line.startsWith("@@")) {
      i = parseHunk(lines, i, section);
    } else if (section && line.startsWith("\\")) {
      throw lineError(i + 1, "misplaced no-newline marker");
    } else {
      i += 1;
    }
  }
  requireHunks(section);
  if (sections.length === 0) throw new PatchError("no file headers found");
  return sections;
}

function requireHunks(section) {
  if (section && section.hunks.length === 0) throw lineError(section.lineNo, "file has no hunks");
}

function parseFileHeader(lines, i) {
  const lineNo = i + 1;
  const next = lines[i + 1];
  if (next === undefined || !next.startsWith("+++ ")) throw lineError(lineNo, "missing +++ header");
  const oldPath = headerPath(lines[i]);
  const newPath = headerPath(next);
  if (oldPath === DEV_NULL && newPath === DEV_NULL) throw lineError(lineNo, "both paths are /dev/null");
  if (oldPath === DEV_NULL) return { lineNo, action: "create", path: newPath, hunks: [] };
  if (newPath === DEV_NULL) return { lineNo, action: "delete", path: oldPath, hunks: [] };
  if (oldPath !== newPath) throw lineError(lineNo, "renames are not supported");
  return { lineNo, action: "modify", path: oldPath, hunks: [] };
}

function headerPath(line) {
  const path = line.slice(4).split(/[\t\r]/, 1)[0];
  if (path === DEV_NULL) return path;
  return path.startsWith("a/") || path.startsWith("b/") ? path.slice(2) : path;
}

/** Parse the hunk whose header is lines[i]; returns the index of the first line after it. */
function parseHunk(lines, i, section) {
  const headerNo = i + 1;
  const match = HUNK_HEADER.exec(lines[i]);
  if (!match) throw lineError(headerNo, "malformed hunk header");
  const [oldStart, oldCount, newStart, newCount] = [match[1], match[2] ?? "1", match[3], match[4] ?? "1"].map(Number);
  if ((oldStart === 0 && oldCount > 0) || (newStart === 0 && newCount > 0)) {
    throw lineError(headerNo, "malformed hunk header");
  }

  const body = [];
  let oldLeft = oldCount;
  let newLeft = newCount;
  i += 1;
  while (oldLeft > 0 || newLeft > 0) {
    if (i >= lines.length) throw lineError(headerNo, "hunk truncated");
    const line = lines[i];
    const kind = line === "" ? " " : line[0];
    if (kind === "\\") throw lineError(i + 1, "misplaced no-newline marker");
    if (kind !== " " && kind !== "-" && kind !== "+") throw lineError(i + 1, "unexpected line in hunk");
    const onOld = kind !== "+";
    const onNew = kind !== "-";
    if ((onOld && oldLeft === 0) || (onNew && newLeft === 0)) {
      throw lineError(i + 1, "hunk line exceeds header counts");
    }
    if (onOld) oldLeft -= 1;
    if (onNew) newLeft -= 1;
    const entry = { kind, text: line.slice(1), noNewline: false };
    body.push(entry);
    i += 1;
    if (i < lines.length && lines[i].startsWith("\\")) {
      if ((onOld && oldLeft > 0) || (onNew && newLeft > 0)) throw lineError(i + 1, "misplaced no-newline marker");
      entry.noNewline = true;
      i += 1;
    }
  }
  section.hunks.push({ oldStart, oldCount, newStart, newCount, body });
  return i;
}

// ---------------------------------------------------------------------------------------------
// Reverse

const INVERTED_ACTION = { create: "delete", delete: "create", modify: "modify" };
const INVERTED_KIND = { " ": " ", "-": "+", "+": "-" };

function invertSection(section) {
  return {
    ...section,
    action: INVERTED_ACTION[section.action],
    hunks: section.hunks.map((hunk) => ({
      oldStart: hunk.newStart,
      oldCount: hunk.newCount,
      newStart: hunk.oldStart,
      newCount: hunk.oldCount,
      body: hunk.body.map((entry) => ({ ...entry, kind: INVERTED_KIND[entry.kind] })),
    })),
  };
}

// ---------------------------------------------------------------------------------------------
// Applying

function splitLines(content) {
  return content.match(/[^\n]*\n|[^\n]+$/g) ?? [];
}

const fullText = (entry) => (entry.noNewline ? entry.text : `${entry.text}\n`);

function applySection(staged, section, maxOffset) {
  const { path, action } = section;
  const exists = staged.has(path);
  if (action === "create" && exists) return { error: `${path} already exists` };
  if (action !== "create" && !exists) return { error: `${path} does not exist` };

  const original = splitLines(exists ? staged.get(path) : "");
  const result = [];
  const offsets = [];
  let min = 0;
  let lastOffset = 0;
  for (const [index, hunk] of section.hunks.entries()) {
    const oldLines = hunk.body.filter((entry) => entry.kind !== "+").map(fullText);
    const newLines = hunk.body.filter((entry) => entry.kind !== "-").map(fullText);
    const stated = hunk.oldCount > 0 ? hunk.oldStart - 1 : hunk.oldStart;
    const position = locate(original, oldLines, stated + lastOffset, min, maxOffset);
    if (position < 0) return { error: `hunk ${index + 1} of ${path} failed` };
    result.push(...original.slice(min, position), ...newLines);
    min = position + oldLines.length;
    lastOffset = position - stated;
    offsets.push(lastOffset);
  }
  result.push(...original.slice(min));
  const content = result.join("");

  if (action === "delete") {
    if (content !== "") return { error: `${path} is not empty after deletion` };
    staged.delete(path);
  } else {
    staged.set(path, content);
  }
  return { path, action, offsets };
}

function locate(lines, oldLines, center, min, maxOffset) {
  for (let distance = 0; distance <= maxOffset; distance += 1) {
    const candidates = distance === 0 ? [center] : [center - distance, center + distance];
    for (const position of candidates) {
      if (matchesAt(lines, oldLines, position, min)) return position;
    }
  }
  return -1;
}

function matchesAt(lines, oldLines, position, min) {
  if (position < min || position + oldLines.length > lines.length) return false;
  return oldLines.every((line, k) => lines[position + k] === line);
}
