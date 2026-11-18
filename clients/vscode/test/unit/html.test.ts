import { strict as assert } from "node:assert";
import { test } from "node:test";
import { csp, html, nonce } from "../../src/extension/html.js";

test("the CSP allows only nonce scripts and the webview's own resources", () => {
  assert.equal(
    csp("vscode-resource:", "N0NCE"),
    "default-src 'none'; script-src 'nonce-N0NCE'; style-src vscode-resource: 'nonce-N0NCE'; img-src vscode-resource: data:; font-src vscode-resource:",
  );
  const page = html({ cspSource: "vscode-resource:", script: "s.js", style: "s.css", nonce: "N0NCE" });
  assert.ok(!page.includes("connect-src"));
  assert.ok(!/https?:\/\//.test(page));
  assert.ok(page.includes(`<script nonce="N0NCE" src="s.js">`));
});

test("each render gets a new random nonce", () => {
  const values = new Set(Array.from({ length: 20 }, nonce));
  assert.equal(values.size, 20);
  for (const value of values) assert.match(value, /^[A-Za-z0-9_-]{24}$/);
});
