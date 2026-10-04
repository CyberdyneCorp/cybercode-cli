# expenses

Expense report helpers (Node 22, ES modules, no dependencies).

`src/csv.js` contract — `parseCsv(text)` returns an array of records, each an array of strings:

- Fields are separated by `,`. Records end with `\n` or `\r\n` (both may appear in one file).
- A field may be wrapped in double quotes. Inside quotes, commas, `\n` and `\r\n` are part of the
  field, and `""` stands for one literal `"`. The wrapping quotes are not part of the value.
- Empty fields are empty strings: `a,,b` is `["a", "", "b"]` and `a,` is `["a", ""]`.
- One final line ending does not create an extra record. Empty input returns `[]`.
- A quoted field that is never closed throws an `Error`.

`src/report.js` contract — `totalsByCategory(text)` parses the CSV (first record is the header,
which contains at least `category` and `amount` columns, in any order) and returns an object
mapping each category to the sum of its `amount` values as numbers.

Exports from the bank contain notes such as `"Lunch, team"` and use Windows line endings, and the
totals come out wrong. Run the tests with `node --test`.
