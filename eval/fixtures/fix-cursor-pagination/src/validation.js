/** Validation of listItems parameters. Checks run in the order documented in README.md. */

import { invalidParam } from "./errors.js";
import { DEFAULT_SORT, parseSort } from "./sort.js";
import { FILTER_NAMES, buildFilters } from "./filters.js";

export const DEFAULT_LIMIT = 20;
export const MAX_LIMIT = 100;

const KNOWN_PARAMS = new Set(["sort", "limit", "cursor", ...FILTER_NAMES]);

function checkKnown(params) {
  for (const name of Object.keys(params)) {
    if (!KNOWN_PARAMS.has(name)) throw invalidParam(`unknown parameter: ${name}`);
  }
}

function checkSort(spec = DEFAULT_SORT) {
  const sort = typeof spec === "string" ? parseSort(spec) : null;
  if (!sort) throw invalidParam("sort must be one of name, price, updatedAt, optionally prefixed with -");
  return sort;
}

function checkLimit(limit = DEFAULT_LIMIT) {
  if (!Number.isInteger(limit) || limit < 1 || limit >= MAX_LIMIT) {
    throw invalidParam(`limit must be an integer between 1 and ${MAX_LIMIT}`);
  }
  return limit;
}

function checkPrice(name, value) {
  if (value !== undefined && !(typeof value === "number" && Number.isFinite(value))) {
    throw invalidParam(`${name} must be a finite number`);
  }
}

function checkFilters(params) {
  if (params.category !== undefined && (typeof params.category !== "string" || params.category === "")) {
    throw invalidParam("category must be a non-empty string");
  }
  checkPrice("minPrice", params.minPrice);
  checkPrice("maxPrice", params.maxPrice);
  if (params.minPrice !== undefined && params.maxPrice !== undefined && params.minPrice > params.maxPrice) {
    throw invalidParam("minPrice must not exceed maxPrice");
  }
  return buildFilters(params);
}

function checkCursor(cursor) {
  if (cursor !== undefined && cursor !== null && typeof cursor !== "string") {
    throw invalidParam("cursor must be a string");
  }
  return cursor ?? null;
}

/** Returns { sort, limit, filters, cursor } or throws ApiError INVALID_PARAM. */
export function validateParams(params) {
  checkKnown(params);
  const sort = checkSort(params.sort);
  const limit = checkLimit(params.limit);
  const filters = checkFilters(params);
  const cursor = checkCursor(params.cursor);
  return { sort, limit, filters, cursor };
}
