/**
 * Opaque pagination cursors.
 *
 * A cursor is base64url(JSON) of
 *   { v: 1, q: <query fingerprint>, d: "next" | "prev", k: { value, id } }
 * where `k` is the sort key of the item the page boundary sits on and `q` binds the cursor
 * to the sort and filters of the request that issued it.
 */

import { invalidCursor, cursorMismatch } from "./errors.js";
import { FILTER_NAMES } from "./filters.js";

const VERSION = 1;

/** Identifies the result set a cursor belongs to: the sort and every filter. */
export function queryFingerprint(sort, filters) {
  return JSON.stringify([sort.spec, ...FILTER_NAMES.map((name) => filters[name] ?? null)]);
}

/** The sort value is stored as is, so timestamps keep their milliseconds. */
export function encodeCursor({ fingerprint, direction, key }) {
  const [value, id] = key;
  const payload = { v: VERSION, q: fingerprint, d: direction, k: { value, id } };
  return Buffer.from(JSON.stringify(payload), "utf8").toString("base64url");
}

function parsePayload(cursor) {
  try {
    return JSON.parse(Buffer.from(cursor, "base64url").toString("utf8"));
  } catch {
    return null;
  }
}

function isWellFormed(payload) {
  return (
    payload !== null &&
    typeof payload === "object" &&
    payload.v === VERSION &&
    typeof payload.q === "string" &&
    (payload.d === "next" || payload.d === "prev") &&
    payload.k !== null &&
    typeof payload.k === "object" &&
    typeof payload.k.id === "string" &&
    (payload.k.value === null || ["string", "number"].includes(typeof payload.k.value))
  );
}

/**
 * Decodes a cursor issued for the query identified by `fingerprint`.
 * Returns { direction: "next" | "prev", key: [value, id] }.
 */
export function decodeCursor(cursor, fingerprint) {
  const payload = parsePayload(cursor);
  if (!isWellFormed(payload)) throw invalidCursor();
  if (payload.q !== fingerprint) throw cursorMismatch();
  return { direction: payload.d, key: [payload.k.value, payload.k.id] };
}
