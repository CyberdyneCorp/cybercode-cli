import { test } from "node:test";
import assert from "node:assert/strict";

import { parseCsv } from "./src/csv.js";
import { totalsByCategory } from "./src/report.js";

test("visible: simple records and totals", () => {
  assert.deepEqual(parseCsv("a,b\n1,2\n"), [["a", "b"], ["1", "2"]]);
  assert.deepEqual(totalsByCategory("category,amount\nfood,10\ntravel,5\nfood,2.5"), { food: 12.5, travel: 5 });
});

test("quoted fields keep commas", () => {
  assert.deepEqual(parseCsv('a,"b,c",d'), [["a", "b,c", "d"]]);
  assert.deepEqual(parseCsv('"x, y, z"'), [["x, y, z"]]);
});

test("doubled quotes are literal quotes", () => {
  assert.deepEqual(parseCsv('"say ""hi""",2'), [['say "hi"', "2"]]);
  assert.deepEqual(parseCsv('""""'), [['"']]);
  assert.deepEqual(parseCsv('"",x'), [["", "x"]]);
});

test("CRLF and mixed line endings", () => {
  assert.deepEqual(parseCsv("a,b\r\n1,2\r\n3,4\n5,6"), [["a", "b"], ["1", "2"], ["3", "4"], ["5", "6"]]);
});

test("one trailing line ending adds no record", () => {
  assert.deepEqual(parseCsv("a\n"), [["a"]]);
  assert.deepEqual(parseCsv("a\r\n"), [["a"]]);
  assert.deepEqual(parseCsv("a,b"), [["a", "b"]]);
});

test("empty input", () => {
  assert.deepEqual(parseCsv(""), []);
});

test("empty fields", () => {
  assert.deepEqual(parseCsv("a,,b"), [["a", "", "b"]]);
  assert.deepEqual(parseCsv("a,"), [["a", ""]]);
  assert.deepEqual(parseCsv(",a"), [["", "a"]]);
  assert.deepEqual(parseCsv(",,\n1,2,3\n"), [["", "", ""], ["1", "2", "3"]]);
});

test("line breaks inside quotes belong to the field", () => {
  assert.deepEqual(parseCsv('id,note\n1,"line one\nline two"\n2,x\n'), [
    ["id", "note"], ["1", "line one\nline two"], ["2", "x"],
  ]);
  assert.deepEqual(parseCsv('1,"a\r\nb"\r\n'), [["1", "a\r\nb"]]);
});

test("unterminated quote throws", () => {
  assert.throws(() => parseCsv('a,"open\n1,2'), Error);
});

test("totals with quotes, CRLF and reordered columns", () => {
  const text = [
    "date,note,amount,category",
    '2024-01-02,"Lunch, team",12.5,food',
    '2024-01-03,"Taxi ""airport""",30,travel',
    '2024-01-04,"multi\r\nline",7.5,food',
    "2024-01-05,,0.25,office",
    "",
  ].join("\r\n");
  assert.deepEqual(totalsByCategory(text), { food: 20, travel: 30, office: 0.25 });
});

test("quoted category names", () => {
  const text = 'category,amount\n"Food, groceries",1\n"Food, groceries",2\n';
  assert.deepEqual(totalsByCategory(text), { "Food, groceries": 3 });
});
