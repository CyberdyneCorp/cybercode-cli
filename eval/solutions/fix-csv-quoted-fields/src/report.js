import { parseCsv } from "./csv.js";

/** Sum the `amount` column per `category`. See README.md for the contract. */
export function totalsByCategory(text) {
  const [columns, ...records] = parseCsv(text);
  const category = columns.indexOf("category");
  const amount = columns.indexOf("amount");
  const totals = {};
  for (const fields of records) {
    totals[fields[category]] = (totals[fields[category]] ?? 0) + Number(fields[amount]);
  }
  return totals;
}
