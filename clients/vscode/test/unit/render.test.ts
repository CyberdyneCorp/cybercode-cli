import { strict as assert } from "node:assert";
import { test } from "node:test";
import { inline, openable, render, type Node } from "../../src/webview/render.js";

function tags(nodes: Node[], out = new Set<string>()): Set<string> {
  for (const node of nodes) if (typeof node !== "string") {
    out.add(node.tag);
    tags(node.children, out);
  }
  return out;
}

function text(nodes: Node[]): string {
  return nodes.map((node) => (typeof node === "string" ? node : text(node.children))).join("");
}

test("Injected HTML rendered inert: tags stay literal text", () => {
  const hostile = '<img src="https://attacker.example/x.png" onerror="alert(1)"><script>x()</script>';
  const nodes = render(hostile);
  assert.deepEqual([...tags(nodes)], ["p"]);
  assert.equal(text(nodes), hostile);
});

test("images are never loaded and links carry their URL as data", () => {
  const nodes = inline("![beacon](https://attacker.example/p.png) and [docs](https://example.com/a)");
  assert.equal(nodes[0], "![beacon](https://attacker.example/p.png)");
  const link = nodes.find((node) => typeof node !== "string" && node.tag === "a");
  assert.deepEqual(link, { tag: "a", children: ["docs"], href: "https://example.com/a" });
});

test("a javascript: link is not openable", () => {
  assert.equal(openable("javascript:alert(1)"), false);
  assert.equal(openable("data:text/html,x"), false);
  assert.equal(openable("https://example.com"), true);
});

test("only allowlisted elements are produced", () => {
  const nodes = render("# Title\n\n> quote\n\n- a\n- **b**\n\n1. one\n\n| x | y |\n|---|---|\n| 1 | 2 |\n\n```js\n<b>code</b>\n```\n\n`inline` *em*");
  const allowed = new Set(["p", "code", "pre", "em", "strong", "ul", "ol", "li", "a", "h1", "h2", "h3", "h4", "blockquote", "table", "thead", "tbody", "tr", "th", "td"]);
  for (const tag of tags(nodes)) assert.ok(allowed.has(tag), tag);
  assert.ok(text(nodes).includes("<b>code</b>"));
});
