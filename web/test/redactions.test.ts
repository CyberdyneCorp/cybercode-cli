import assert from "node:assert/strict";
import { test } from "node:test";
import { redactionNote } from "../src/redactions.js";

test("a tool that hid secrets says how many, and one that hid none says nothing", () => {
  assert.equal(redactionNote({ redactions: { count: 2 } }), "2 secrets hidden from the model");
  assert.equal(redactionNote({ redactions: { count: 1 } }), "1 secret hidden from the model");
  assert.equal(redactionNote({ redactions: null }), undefined);
  assert.equal(redactionNote({}), undefined);
});
