import assert from "node:assert/strict";
import { test } from "node:test";
import { View } from "../src/view.js";

test("image, PDF and attachment references render only inert deferred-attachment text", () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "document");
  const created: string[] = [];
  Object.defineProperty(globalThis, "document", { configurable: true, value: {
    createElement(tag: string) { created.push(tag); return { className: "", textContent: "" }; },
  } });
  try {
    const view = new View(undefined as never, undefined as never) as unknown as { part(value: unknown): { textContent: string } };
    for (const type of ["image", "document", "attachment"]) {
      // Access to raw inline bytes or attachment metadata is forbidden in this slice.
      const content = new Proxy({ type }, { get(target, key) { if (key !== "type") throw new Error(`read attachment field ${String(key)}`); return target.type; } });
      const element = view.part(content);
      assert.match(element.textContent, /^Attachment · display is outside this dashboard slice$/);
      assert.ok(!element.textContent.includes("Reasoning"));
    }
    assert.deepEqual(created, ["p", "p", "p"]); // No image, object, embed, link or beacon elements.
  } finally {
    if (previous) Object.defineProperty(globalThis, "document", previous);
    else Reflect.deleteProperty(globalThis, "document");
  }
});
