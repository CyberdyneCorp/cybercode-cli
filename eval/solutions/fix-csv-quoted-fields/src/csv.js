/**
 * Parse CSV text into records (arrays of strings). See README.md for the contract.
 * A single pass over the characters, tracking whether we are inside a quoted field.
 */
export function parseCsv(text) {
  const records = [];
  let record = [];
  let field = "";
  let inQuotes = false;
  let i = 0;

  const endField = () => {
    record.push(field);
    field = "";
  };
  const endRecord = () => {
    endField();
    records.push(record);
    record = [];
  };

  while (i < text.length) {
    const ch = text[i];
    if (inQuotes) {
      if (ch === '"' && text[i + 1] === '"') {
        field += '"';
        i += 2;
        continue;
      }
      if (ch === '"') inQuotes = false;
      else field += ch;
      i += 1;
    } else if (ch === '"') {
      inQuotes = true;
      i += 1;
    } else if (ch === ",") {
      endField();
      i += 1;
    } else if (ch === "\n" || (ch === "\r" && text[i + 1] === "\n")) {
      endRecord();
      i += ch === "\r" ? 2 : 1;
    } else {
      field += ch;
      i += 1;
    }
  }
  if (inQuotes) throw new Error("unterminated quoted field");
  if (field !== "" || record.length > 0) endRecord();
  return records;
}
