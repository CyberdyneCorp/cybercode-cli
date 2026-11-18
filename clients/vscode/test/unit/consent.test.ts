import { strict as assert } from "node:assert";
import { test } from "node:test";
import { answer, decide } from "../../src/extension/consent.js";

test("Asked once: Always start starts and saves always, so later windows start without asking", () => {
  assert.equal(decide(undefined, false), "ask");
  assert.deepEqual(answer("Always start"), { start: true, save: "always" });
  assert.equal(decide("always", false), "start");
});

test("Declined: Don't start starts nothing and saves never; the panel offers Start server", () => {
  assert.deepEqual(answer("Don't start"), { start: false, save: "never" });
  assert.equal(decide("never", false), "wait");
  assert.deepEqual(answer(undefined), { start: false }, "a dismissed modal saves nothing");
});

test("Start this time saves nothing, and the question comes back in the next window", () => {
  assert.deepEqual(answer("Start this time"), { start: true });
  assert.equal(decide("ask", true), "wait", "asked once per window");
  assert.equal(decide("ask", false), "ask", "Changed in settings: ask again");
});
