import { parseLine } from "./csv.js";

/** Sum the `amount` column per `category`. See README.md for the contract. */
export function totalsByCategory(text) {
  const [header, ...lines] = text.trim().split("\n");
  const columns = parseLine(header);
  const category = columns.indexOf("category");
  const amount = columns.indexOf("amount");
  const totals = {};
  for (const line of lines) {
    const fields = parseLine(line);
    totals[fields[category]] = (totals[fields[category]] ?? 0) + Number(fields[amount]);
  }
  return totals;
}
