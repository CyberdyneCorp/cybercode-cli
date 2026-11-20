import { strict as assert } from "node:assert";
import { test } from "node:test";
import { expiresSoon, parseTokenFile } from "../../src/extension/token.js";

const token = `cyi_${"A".repeat(43)}`;

test("the token file holds the token and an optional expiry", () => {
  assert.deepEqual(parseTokenFile(`${token}\n`), { token });
  const parsed = parseTokenFile(`${token}\nexpires_at=2026-12-18T10:00:00Z\n`);
  assert.equal(parsed?.expiresAt, Date.parse("2026-12-18T10:00:00Z"));
  for (const bad of ["", "cyi_short", `Bearer ${token}`, `${token}x`, "password"]) assert.equal(parseTokenFile(bad), undefined, bad);
});

test("a token expiring within 7 days is flagged", () => {
  const now = Date.parse("2026-12-12T10:00:00Z");
  assert.equal(expiresSoon({ token, expiresAt: Date.parse("2026-12-18T10:00:00Z") }, now), true);
  assert.equal(expiresSoon({ token, expiresAt: Date.parse("2027-01-18T10:00:00Z") }, now), false);
  assert.equal(expiresSoon({ token }, now), false);
});
