// Hidden tests for fix-cursor-pagination. Copied into the workspace root by grade.py.
import { test } from "node:test";
import assert from "node:assert/strict";

import { ItemStore, createApi, ApiError } from "./src/index.js";

// ---------------------------------------------------------------- helpers

function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const pick = (rand, list) => list[Math.floor(rand() * list.length)];

const NAMES = ["", "a", "A", "b", "B", "Z", "é", "ä", "apple", "Apple", "\u{1F600}", "～", null, null, "a"];
const PRICES = [0, 0, 0.1, 1.5, 2, 2, 2, 10, null, null];
const TIMES = [
  "2024-03-01T12:00:00.000Z",
  "2024-03-01T12:00:00.001Z",
  "2024-03-01T12:00:00.500Z",
  "2024-03-01T12:00:00.999Z",
  "2024-03-01T12:00:01.000Z",
  "2023-12-31T23:59:59.999Z",
  null,
  null,
];
const CATEGORIES = ["home", "office"];
const ID_CHARS = "aAbBzZ09_-";

function makeItem(rand, used) {
  let id;
  do {
    id = "";
    const len = 1 + Math.floor(rand() * 3);
    for (let i = 0; i < len; i++) id += pick(rand, ID_CHARS);
  } while (used.has(id));
  used.add(id);
  return { id, name: pick(rand, NAMES), price: pick(rand, PRICES), updatedAt: pick(rand, TIMES), category: pick(rand, CATEGORIES) };
}

function dataset(seed, n) {
  const rand = rng(seed);
  const used = new Set();
  return { rand, used, items: Array.from({ length: n }, () => makeItem(rand, used)) };
}

const SORTS = ["name", "-name", "price", "-price", "updatedAt", "-updatedAt"];

// Reference ordering, written straight from README "Ordering".
function cmpStr(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}
function refCompare(sortSpec) {
  const desc = sortSpec.startsWith("-");
  const field = desc ? sortSpec.slice(1) : sortSpec;
  return (a, b) => {
    const va = a[field] ?? null;
    const vb = b[field] ?? null;
    if ((va === null) !== (vb === null)) return va === null ? 1 : -1;
    let c = 0;
    if (va !== null) c = field === "price" ? Math.sign(va - vb) : cmpStr(va, vb);
    if (c === 0) c = cmpStr(a.id, b.id);
    return desc ? -c : c;
  };
}
function refMatches(item, params) {
  if (params.category !== undefined && item.category !== params.category) return false;
  const priced = params.minPrice !== undefined || params.maxPrice !== undefined;
  if (priced && (item.price ?? null) === null) return false;
  if (params.minPrice !== undefined && item.price < params.minPrice) return false;
  if (params.maxPrice !== undefined && item.price > params.maxPrice) return false;
  return true;
}
function refRows(items, params) {
  return items.filter((item) => refMatches(item, params)).sort(refCompare(params.sort ?? "updatedAt"));
}

const ids = (items) => items.map((item) => item.id);

function chunks(list, size) {
  const out = [];
  for (let i = 0; i < list.length; i += size) out.push(list.slice(i, i + size));
  return out;
}

function throwsApi(fn, code, message, context) {
  assert.throws(fn, (error) => {
    assert.ok(error instanceof ApiError, `${context}: expected ApiError, got ${error}`);
    assert.equal(error.code, code, `${context}: code`);
    assert.equal(error.message, message, `${context}: message`);
    assert.equal(error.status, 400, `${context}: status`);
    return true;
  }, context);
}

/** Forward walk; checks every page and cursor against the reference. Returns the pages. */
function walkForward(api, store, params, context) {
  const expected = chunks(refRows(store.all(), params), params.limit);
  const pages = [];
  let cursor = null;
  for (let i = 0; i < 1000; i++) {
    const page = api.listItems({ ...params, cursor });
    pages.push(page);
    const want = expected[i] ?? [];
    const where = `${context} forward page ${i + 1}`;
    assert.deepEqual(ids(page.items), ids(want), `${where}: items`);
    assert.equal(page.prevCursor === null, i === 0 || want.length === 0, `${where}: prevCursor presence`);
    assert.equal(page.nextCursor === null, i >= expected.length - 1, `${where}: nextCursor presence`);
    if (!page.nextCursor) break;
    cursor = page.nextCursor;
  }
  return pages;
}

// ---------------------------------------------------------------- validation

const BAD = "INVALID_PARAM";
const SORT_MSG = "sort must be one of name, price, updatedAt, optionally prefixed with -";
const LIMIT_MSG = "limit must be an integer between 1 and 100";

test("parameter validation messages and order", () => {
  const api = createApi(new ItemStore([]));
  const cases = [
    [{ foo: 1 }, "unknown parameter: foo"],
    [{ foo: 1, sort: "nope" }, "unknown parameter: foo"],
    [{ sort: "nope" }, SORT_MSG],
    [{ sort: "Name" }, SORT_MSG],
    [{ sort: "--price" }, SORT_MSG],
    [{ sort: "+price" }, SORT_MSG],
    [{ sort: "id" }, SORT_MSG],
    [{ sort: 1 }, SORT_MSG],
    [{ sort: "nope", limit: 0 }, SORT_MSG],
    [{ limit: 0 }, LIMIT_MSG],
    [{ limit: 101 }, LIMIT_MSG],
    [{ limit: 2.5 }, LIMIT_MSG],
    [{ limit: "5" }, LIMIT_MSG],
    [{ limit: -1 }, LIMIT_MSG],
    [{ limit: NaN }, LIMIT_MSG],
    [{ limit: 0, category: "" }, LIMIT_MSG],
    [{ category: "" }, "category must be a non-empty string"],
    [{ category: 3 }, "category must be a non-empty string"],
    [{ category: "", minPrice: "x" }, "category must be a non-empty string"],
    [{ minPrice: "3" }, "minPrice must be a finite number"],
    [{ minPrice: Infinity }, "minPrice must be a finite number"],
    [{ maxPrice: NaN }, "maxPrice must be a finite number"],
    [{ minPrice: 5, maxPrice: 4 }, "minPrice must not exceed maxPrice"],
    [{ cursor: 5 }, "cursor must be a string"],
    [{ cursor: {}, minPrice: 5, maxPrice: 4 }, "minPrice must not exceed maxPrice"],
  ];
  for (const [params, message] of cases) {
    throwsApi(() => api.listItems(params), BAD, message, `listItems(${JSON.stringify(params)})`);
  }
});

test("limit 1 and 100 are accepted, default limit is 20", () => {
  const { items } = dataset(7, 130);
  const api = createApi(new ItemStore(items));
  assert.equal(api.listItems({ limit: 100 }).items.length, 100, "limit 100");
  assert.equal(api.listItems({ limit: 1 }).items.length, 1, "limit 1");
  assert.equal(api.listItems({}).items.length, 20, "default limit");
  assert.equal(api.listItems().items.length, 20, "no params");
  assert.equal(api.listItems({ minPrice: 2, maxPrice: 2, limit: 100 }).items.every((i) => i.price === 2), true);
});

test("undefined params are the same as absent params", () => {
  const { items } = dataset(8, 30);
  const api = createApi(new ItemStore(items));
  const a = api.listItems({ limit: 5, sort: undefined, category: undefined, minPrice: undefined, cursor: undefined });
  const b = api.listItems({ limit: 5 });
  assert.deepEqual(ids(a.items), ids(b.items));
  const next = api.listItems({ limit: 5, category: undefined, maxPrice: undefined, cursor: b.nextCursor });
  assert.deepEqual(ids(next.items), ids(refRows(items, { limit: 5 }).slice(5, 10)), "cursor accepted with undefined filters");
});

// ---------------------------------------------------------------- cursors

test("strings that are not cursors give INVALID_CURSOR", () => {
  const api = createApi(new ItemStore(dataset(9, 10).items));
  const b64 = (value) => Buffer.from(typeof value === "string" ? value : JSON.stringify(value)).toString("base64url");
  const bad = [
    "not-a-cursor",
    "",
    "%%%",
    b64("hello"),
    b64({ a: 1 }),
    b64([1, 2]),
    b64(null),
    b64({ v: 2, q: "x", d: "next", k: { value: 1, id: "a" } }),
    b64({ v: 1, q: "x", d: "sideways", k: { value: 1, id: "a" } }),
    b64({ v: 1, q: "x", d: "next", k: { value: 1 } }),
    b64({ v: 1, q: "x", d: "next" }),
  ];
  for (const cursor of bad) {
    throwsApi(() => api.listItems({ cursor }), "INVALID_CURSOR", "invalid cursor", `cursor ${JSON.stringify(cursor)}`);
  }
});

test("cursors are bound to sort and every filter", () => {
  const { items } = dataset(10, 40);
  const api = createApi(new ItemStore(items));
  const base = { sort: "price", category: "home", minPrice: 0, maxPrice: 10, limit: 3 };
  const first = api.listItems(base);
  assert.ok(first.nextCursor, "first page has a next cursor");
  const second = api.listItems({ ...base, cursor: first.nextCursor });
  assert.ok(second.prevCursor, "second page has a prev cursor");
  const variants = [
    { sort: "-price" },
    { sort: "name" },
    { sort: undefined },
    { category: "office" },
    { category: undefined },
    { minPrice: 0.1 },
    { minPrice: undefined },
    { maxPrice: 9 },
    { maxPrice: undefined },
  ];
  for (const change of variants) {
    for (const cursor of [first.nextCursor, second.prevCursor]) {
      throwsApi(
        () => api.listItems({ ...base, ...change, cursor }),
        "CURSOR_MISMATCH",
        "cursor does not match the current sort and filters",
        `cursor reused with ${JSON.stringify(change)}`,
      );
    }
  }
  const plain = api.listItems({ limit: 2 });
  throwsApi(() => api.listItems({ limit: 2, category: "home", cursor: plain.nextCursor }), "CURSOR_MISMATCH",
    "cursor does not match the current sort and filters", "filter added to an unfiltered cursor");
  throwsApi(() => api.listItems({ limit: 2, minPrice: 0, cursor: plain.nextCursor }), "CURSOR_MISMATCH",
    "cursor does not match the current sort and filters", "price filter added to an unfiltered cursor");
  // limit may change
  const rows = refRows(items, base);
  assert.deepEqual(ids(api.listItems({ ...base, limit: 5, cursor: first.nextCursor }).items), ids(rows.slice(3, 8)));
});

test("updatedAt cursors keep milliseconds", () => {
  const items = ["000", "001", "500", "999"].flatMap((ms, i) => [
    { id: `x${i}`, updatedAt: `2024-03-01T12:00:00.${ms}Z`, category: "c" },
    { id: `y${i}`, updatedAt: `2024-03-01T12:00:00.${ms}Z`, category: "c" },
  ]);
  for (const sort of ["updatedAt", "-updatedAt"]) {
    for (let limit = 1; limit <= items.length; limit++) {
      const store = new ItemStore(items);
      walkForward(createApi(store), store, { sort, limit }, `${sort} limit ${limit}`);
    }
  }
});

// ---------------------------------------------------------------- ordering

test("ordering follows README: code units, nulls last both ways, id tie-break", () => {
  const items = [
    { id: "n1", name: null, price: null, updatedAt: null },
    { id: "n0", name: null, price: null, updatedAt: null },
    { id: "e", name: "", price: 0, updatedAt: "2024-01-01T00:00:00.000Z" },
    { id: "b", name: "B", price: 2, updatedAt: "2024-01-01T00:00:00.001Z" },
    { id: "a", name: "a", price: 2, updatedAt: "2024-01-01T00:00:00.001Z" },
    { id: "z", name: "é", price: 0.5, updatedAt: "2023-06-01T00:00:00.000Z" },
    { id: "s", name: "\u{1F600}", price: 100, updatedAt: "2025-01-01T00:00:00.000Z" },
    { id: "w", name: "～", price: 2, updatedAt: "2024-01-01T00:00:00.000Z" },
  ];
  const api = createApi(new ItemStore(items));
  const order = (sort) => ids(api.listItems({ sort, limit: 100 }).items);
  assert.deepEqual(order("name"), ["e", "b", "a", "z", "s", "w", "n0", "n1"], "name");
  assert.deepEqual(order("-name"), ["w", "s", "z", "a", "b", "e", "n1", "n0"], "-name");
  assert.deepEqual(order("price"), ["e", "z", "a", "b", "w", "s", "n0", "n1"], "price");
  assert.deepEqual(order("-price"), ["s", "w", "b", "a", "z", "e", "n1", "n0"], "-price");
  assert.deepEqual(order("updatedAt"), ["z", "e", "w", "a", "b", "s", "n0", "n1"], "updatedAt");
  assert.deepEqual(order("-updatedAt"), ["s", "b", "a", "w", "e", "z", "n1", "n0"], "-updatedAt");
  assert.deepEqual(order(undefined), order("updatedAt"), "default sort");
});

test("filters: category exact, inclusive price range, null prices excluded", () => {
  const { items } = dataset(11, 60);
  const api = createApi(new ItemStore(items));
  const cases = [{ category: "home" }, { category: "Home" }, { minPrice: 0 }, { maxPrice: 2 }, { minPrice: 2, maxPrice: 2 },
    { minPrice: 0.1, maxPrice: 1.5, category: "office" }, { minPrice: 1000 }];
  for (const filters of cases) {
    for (const sort of SORTS) {
      const params = { ...filters, sort, limit: 100 };
      assert.deepEqual(ids(api.listItems(params).items), ids(refRows(items, params)), JSON.stringify(params));
    }
  }
});

// ---------------------------------------------------------------- walks

function backwardFromEnd(api, store, params, forwardPages, context) {
  const expected = forwardPages.map((page) => ids(page.items)).reverse();
  let cursor = forwardPages[forwardPages.length - 1].prevCursor;
  const got = [ids(forwardPages[forwardPages.length - 1].items)];
  for (let i = 1; cursor && i < 1000; i++) {
    const page = api.listItems({ ...params, cursor });
    const where = `${context} backward page ${i + 1}`;
    assert.deepEqual(ids(page.items), expected[i], `${where}: items`);
    assert.ok(page.nextCursor, `${where}: nextCursor presence`);
    assert.equal(page.prevCursor === null, i === expected.length - 1, `${where}: prevCursor presence`);
    got.push(ids(page.items));
    cursor = page.prevCursor;
  }
  assert.deepEqual(got, expected, `${context}: backward walk yields the forward pages reversed`);
}

function walkAllLimits(seed, n, filters) {
  const { items } = dataset(seed, n);
  for (const sort of SORTS) {
    for (let limit = 1; limit <= n + 1; limit++) {
      const store = new ItemStore(items);
      const api = createApi(store);
      const params = { sort, limit, ...filters };
      const context = `seed ${seed} ${JSON.stringify(params)}`;
      const pages = walkForward(api, store, params, context);
      if (pages[0].items.length) backwardFromEnd(api, store, params, pages, context);
    }
  }
}

for (const [seed, n] of [[1, 12], [2, 20], [3, 25], [4, 7], [5, 30], [6, 1]]) {
  test(`forward and backward walks, every sort and page size (seed ${seed}, ${n} items)`, () => walkAllLimits(seed, n, {}));
}
for (const [seed, filters] of [[21, { category: "home" }], [22, { minPrice: 0, maxPrice: 2 }], [23, { category: "office", minPrice: 0.1 }]]) {
  test(`filtered walks, every sort and page size (seed ${seed}, ${JSON.stringify(filters)})`, () => walkAllLimits(seed, 24, filters));
}

test("prevCursor from any page returns exactly the previous forward page", () => {
  const { items } = dataset(31, 23);
  for (const sort of SORTS) {
    for (const limit of [1, 2, 3, 5, 8]) {
      const store = new ItemStore(items);
      const api = createApi(store);
      const pages = walkForward(api, store, { sort, limit }, `${sort} ${limit}`);
      for (let i = 1; i < pages.length; i++) {
        const back = api.listItems({ sort, limit, cursor: pages[i].prevCursor });
        assert.deepEqual(ids(back.items), ids(pages[i - 1].items), `${sort} limit ${limit}: prev of page ${i + 1}`);
      }
    }
  }
});

test("limit may change during a walk", () => {
  const { items, rand } = dataset(32, 40);
  for (const sort of SORTS) {
    const api = createApi(new ItemStore(items));
    const rows = refRows(items, { sort });
    let offset = 0;
    let cursor = null;
    while (true) {
      const limit = 1 + Math.floor(rand() * 6);
      const page = api.listItems({ sort, limit, cursor });
      assert.deepEqual(ids(page.items), ids(rows.slice(offset, offset + limit)), `${sort} at offset ${offset}`);
      offset += page.items.length;
      if (!page.nextCursor) break;
      cursor = page.nextCursor;
    }
    assert.equal(offset, rows.length, `${sort}: walked every item`);
  }
});

// ---------------------------------------------------------------- mutations between pages

function mutate(rand, store, used, live) {
  const deletions = Math.floor(rand() * 3);
  for (let i = 0; i < deletions && live.size; i++) {
    const id = pick(rand, [...live]);
    store.delete(id);
    live.delete(id);
  }
  const insertions = Math.floor(rand() * 3);
  for (let i = 0; i < insertions; i++) {
    const item = makeItem(rand, used);
    store.insert(item);
    live.add(item.id);
  }
}

function mutatingWalk(seed, sort, limit, filters, backward) {
  const { items, rand, used } = dataset(seed, 25);
  const store = new ItemStore(items);
  const api = createApi(store);
  const live = new Set(items.map((item) => item.id));
  const initial = new Set(live);
  const params = { sort, limit, ...filters };
  const cmp = refCompare(sort);
  const context = `seed ${seed} ${JSON.stringify(params)}${backward ? " backward" : ""}`;
  const seen = [];

  let page;
  if (backward) {
    // reach the last page first, without mutations
    page = api.listItems(params);
    while (page.nextCursor) page = api.listItems({ ...params, cursor: page.nextCursor });
  } else {
    page = api.listItems(params);
  }
  seen.push(...ids(page.items));
  for (let step = 0; step < 200; step++) {
    const cursor = backward ? page.prevCursor : page.nextCursor;
    if (!cursor) break;
    const boundary = backward ? page.items[0] : page.items[page.items.length - 1];
    mutate(rand, store, used, live);
    for (const id of initial) if (!live.has(id)) initial.delete(id);
    page = api.listItems({ ...params, cursor });
    const rows = refRows(store.all(), params);
    const want = backward
      ? rows.filter((row) => cmp(row, boundary) < 0).slice(-limit)
      : rows.filter((row) => cmp(row, boundary) > 0).slice(0, limit);
    const where = `${context} step ${step + 1}`;
    assert.deepEqual(ids(page.items), ids(want), `${where}: items`);
    if (want.length === 0) {
      assert.equal(page.nextCursor, null, `${where}: nextCursor of empty page`);
      assert.equal(page.prevCursor, null, `${where}: prevCursor of empty page`);
      break;
    }
    const first = want[0];
    const last = want[want.length - 1];
    assert.equal(page.nextCursor !== null, rows.some((row) => cmp(row, last) > 0), `${where}: nextCursor presence`);
    assert.equal(page.prevCursor !== null, rows.some((row) => cmp(row, first) < 0), `${where}: prevCursor presence`);
    seen.push(...ids(page.items));
  }
  // every item present for the whole walk was seen exactly once
  for (const id of initial) {
    if (!refMatches(store.get(id), params)) continue;
    assert.equal(seen.filter((s) => s === id).length, 1, `${context}: item ${id} seen once`);
  }
}

test("inserts and deletes between forward pages (randomized)", () => {
  let seed = 100;
  for (const sort of SORTS) {
    for (const limit of [1, 2, 3, 4, 7]) {
      for (const filters of [{}, { category: "home" }, { minPrice: 0 }]) mutatingWalk(seed++, sort, limit, filters, false);
    }
  }
});

test("inserts and deletes between backward pages (randomized)", () => {
  let seed = 500;
  for (const sort of SORTS) {
    for (const limit of [1, 2, 3, 4, 7]) {
      for (const filters of [{}, { category: "office" }, { maxPrice: 2 }]) mutatingWalk(seed++, sort, limit, filters, true);
    }
  }
});

test("an insert and a delete between pages are both visible on the next page", () => {
  const items = ["a", "b", "c", "d", "e", "f"].map((id, i) => ({ id, price: i, category: "c" }));
  const store = new ItemStore(items);
  const api = createApi(store);
  const first = api.listItems({ sort: "price", limit: 2 });
  assert.deepEqual(ids(first.items), ["a", "b"]);
  store.delete("c");
  store.insert({ id: "bb", price: 1.5, category: "c" });
  const second = api.listItems({ sort: "price", limit: 2, cursor: first.nextCursor });
  assert.deepEqual(ids(second.items), ["bb", "d"], "page reflects the store at request time");
});

test("deleting the boundary item does not break its cursor", () => {
  const items = ["a", "b", "c", "d", "e"].map((id) => ({ id, price: 5, category: "c" }));
  const store = new ItemStore(items);
  const api = createApi(store);
  const first = api.listItems({ sort: "-price", limit: 2 });
  assert.deepEqual(ids(first.items), ["e", "d"]);
  store.delete("d");
  const second = api.listItems({ sort: "-price", limit: 2, cursor: first.nextCursor });
  assert.deepEqual(ids(second.items), ["c", "b"]);
  store.delete("c");
  const back = api.listItems({ sort: "-price", limit: 2, cursor: second.prevCursor });
  assert.deepEqual(ids(back.items), ["e"], "prev page after deleting the boundary item");
  assert.equal(back.prevCursor, null);
});

test("prevCursor is null when nothing sorts before the page any more", () => {
  const items = ["a", "b", "c", "d"].map((id, i) => ({ id, name: `n${i}`, category: "c" }));
  const store = new ItemStore(items);
  const api = createApi(store);
  const first = api.listItems({ sort: "name", limit: 2 });
  assert.equal(first.prevCursor, null, "first page");
  store.delete("a");
  store.delete("b");
  const second = api.listItems({ sort: "name", limit: 2, cursor: first.nextCursor });
  assert.deepEqual(ids(second.items), ["c", "d"]);
  assert.equal(second.prevCursor, null, "no item before c any more");
  assert.equal(second.nextCursor, null);
});

test("empty pages have no cursors", () => {
  const store = new ItemStore([{ id: "a", price: 1 }, { id: "b", price: 2 }]);
  const api = createApi(store);
  assert.deepEqual(api.listItems({ category: "none" }), { items: [], nextCursor: null, prevCursor: null });
  const first = api.listItems({ sort: "price", limit: 1 });
  store.delete("b");
  const next = api.listItems({ sort: "price", limit: 1, cursor: first.nextCursor });
  assert.deepEqual(next, { items: [], nextCursor: null, prevCursor: null });
});
