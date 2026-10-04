import { test } from "node:test";
import assert from "node:assert/strict";

import { render } from "../src/render.js";

test("interpolates and escapes a name", () => {
  assert.equal(render("Hello, {{name}}!", { name: "<Ada>" }), "Hello, &lt;Ada&gt;!");
});

test("renders a list section", () => {
  assert.equal(render("{{#items}}[{{.}}]{{/items}}", { items: [1, 2, 3] }), "[1][2][3]");
});
