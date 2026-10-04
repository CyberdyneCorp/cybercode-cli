/** Split one CSV line into fields. */
export function parseLine(line) {
  return line.split(",");
}

/** Parse CSV text into records (arrays of strings). See README.md for the contract. */
export function parseCsv(text) {
  return text.split("\n").map(parseLine);
}
