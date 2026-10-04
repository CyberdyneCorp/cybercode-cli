/** JSON Lines helpers (synchronous). */

/**
 * Parse JSON Lines text. Blank lines (only whitespace) are skipped. A line that is not valid
 * JSON, or whose value is not a plain object, is reported in `malformed` with its 1-based
 * line number and its text.
 */
export function parseLines(text) {
  const records = [];
  const malformed = [];
  text.split("\n").forEach((line, index) => {
    if (line.trim() === "") return;
    let value;
    try {
      value = JSON.parse(line);
    } catch {
      malformed.push({ line: index + 1, text: line });
      return;
    }
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
      malformed.push({ line: index + 1, text: line });
      return;
    }
    records.push(value);
  });
  return { records, malformed };
}

/** One JSON document per line, each line terminated by "\n". */
export function serializeLines(records) {
  return records.map((record) => `${JSON.stringify(record)}\n`).join("");
}
