/**
 * Inert Markdown (`editor-integration` → Extension webview isolation): model, tool and
 * file text becomes a tree of a fixed element allowlist whose leaves are text. Nothing is
 * parsed as HTML, no image is loaded, and a link is a node carrying its URL as data; the
 * extension host opens it only after a confirmation.
 */

export type Tag = "p" | "code" | "pre" | "em" | "strong" | "ul" | "ol" | "li" | "a" | "h1" | "h2" | "h3" | "h4" | "blockquote" | "table" | "thead" | "tbody" | "tr" | "th" | "td";

export type Node = string | { tag: Tag; children: Node[]; href?: string };

const el = (tag: Tag, children: Node[], href?: string): Node => (href === undefined ? { tag, children } : { tag, children, href });

const INLINE = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*\s][^*]*\*|_[^_\s][^_]*_)|(!?\[[^\]]*\]\([^)\s]*\))/;

/** Inline spans: code, strong, emphasis and links; images stay literal text. */
export function inline(text: string): Node[] {
  const out: Node[] = [];
  let rest = text;
  while (rest) {
    const match = INLINE.exec(rest);
    if (!match) break;
    if (match.index > 0) out.push(rest.slice(0, match.index));
    out.push(span(match[0]));
    rest = rest.slice(match.index + match[0].length);
  }
  if (rest) out.push(rest);
  return out;
}

function span(token: string): Node {
  if (token.startsWith("`")) return el("code", [token.slice(1, -1)]);
  if (token.startsWith("**")) return el("strong", inline(token.slice(2, -2)));
  if (token.startsWith("!")) return token;
  if (token.startsWith("[")) {
    const close = token.indexOf("](");
    const label = token.slice(1, close);
    const href = token.slice(close + 2, -1);
    return el("a", [label || href], href);
  }
  return el("em", inline(token.slice(1, -1)));
}

interface Cursor {
  lines: string[];
  at: number;
}

const HEADING = /^(#{1,4})\s+(.*)$/;
const BULLET = /^\s*[-*+]\s+(.*)$/;
const NUMBERED = /^\s*\d+[.)]\s+(.*)$/;
const TABLE_RULE = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

/** Block structure of `text`. */
export function render(text: string): Node[] {
  const cursor: Cursor = { lines: text.replace(/\r\n/g, "\n").split("\n"), at: 0 };
  const out: Node[] = [];
  while (cursor.at < cursor.lines.length) {
    const block = next(cursor);
    if (block) out.push(block);
  }
  return out;
}

function next(cursor: Cursor): Node | undefined {
  const line = cursor.lines[cursor.at] ?? "";
  if (line.trim() === "") {
    cursor.at++;
    return undefined;
  }
  if (line.startsWith("```")) return fence(cursor);
  const heading = HEADING.exec(line);
  if (heading) {
    cursor.at++;
    return el(`h${heading[1]!.length}` as Tag, inline(heading[2] ?? ""));
  }
  if (line.startsWith(">")) return quote(cursor);
  if (BULLET.test(line)) return list(cursor, "ul", BULLET);
  if (NUMBERED.test(line)) return list(cursor, "ol", NUMBERED);
  if (line.includes("|") && TABLE_RULE.test(cursor.lines[cursor.at + 1] ?? "")) return table(cursor);
  return paragraph(cursor);
}

function fence(cursor: Cursor): Node {
  const body: string[] = [];
  cursor.at++;
  while (cursor.at < cursor.lines.length && !(cursor.lines[cursor.at] ?? "").startsWith("```")) {
    body.push(cursor.lines[cursor.at] ?? "");
    cursor.at++;
  }
  cursor.at++;
  return el("pre", [el("code", [body.join("\n")])]);
}

function take(cursor: Cursor, matches: (line: string) => boolean): string[] {
  const taken: string[] = [];
  while (cursor.at < cursor.lines.length && matches(cursor.lines[cursor.at] ?? "")) {
    taken.push(cursor.lines[cursor.at] ?? "");
    cursor.at++;
  }
  return taken;
}

function quote(cursor: Cursor): Node {
  const lines = take(cursor, (line) => line.startsWith(">")).map((line) => line.replace(/^>\s?/, ""));
  return el("blockquote", render(lines.join("\n")));
}

function list(cursor: Cursor, tag: "ul" | "ol", item: RegExp): Node {
  const items = take(cursor, (line) => item.test(line)).map((line) => el("li", inline(item.exec(line)?.[1] ?? "")));
  return el(tag, items);
}

function cells(line: string): string[] {
  return line.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((cell) => cell.trim());
}

function table(cursor: Cursor): Node {
  const head = cells(cursor.lines[cursor.at] ?? "");
  cursor.at += 2;
  const rows = take(cursor, (line) => line.includes("|") && line.trim() !== "");
  return el("table", [
    el("thead", [el("tr", head.map((cell) => el("th", inline(cell))))]),
    el("tbody", rows.map((row) => el("tr", cells(row).map((cell) => el("td", inline(cell)))))),
  ]);
}

function paragraph(cursor: Cursor): Node {
  const start = cursor.at;
  const lines = take(cursor, (line) => line.trim() !== "" && !line.startsWith("```") && !HEADING.test(line) && !BULLET.test(line) && !NUMBERED.test(line) && !line.startsWith(">"));
  if (cursor.at === start) cursor.at++;
  return el("p", inline(lines.join("\n") || (cursor.lines[start] ?? "")));
}

/** Only `http:` and `https:` links may be offered for opening. */
export function openable(href: string): boolean {
  try {
    const url = new URL(href);
    return url.protocol === "http:" || url.protocol === "https:";
  } catch {
    return false;
  }
}
