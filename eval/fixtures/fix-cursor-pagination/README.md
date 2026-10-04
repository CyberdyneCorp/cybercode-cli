# catalog-api

The list endpoint of a small catalog service, as an in-memory Node 22 library (ES modules, no
dependencies). Run the tests with `node --test`.

```js
import { ItemStore, createApi, ApiError } from "./src/index.js";

const store = new ItemStore([{ id: "a1", name: "Lamp", price: 25, updatedAt: "2024-05-01T10:00:00.250Z", category: "home" }]);
const api = createApi(store);
const page = api.listItems({ sort: "-price", limit: 10, category: "home" });
// { items: [...], nextCursor: "eyJ2Ijox..." | null, prevCursor: ... | null }
api.listItems({ sort: "-price", limit: 10, category: "home", cursor: page.nextCursor });
```

## Modules

| File | Role |
|---|---|
| `src/store.js` | `ItemStore`: `insert(item)`, `delete(id)`, `get(id)`, `all()`, `size` |
| `src/sort.js` | sort specs and the item ordering |
| `src/filters.js` | category / price filters |
| `src/repository.js` | sorted, filtered views of the store |
| `src/query.js` | query builder and keyset predicates |
| `src/cursor.js` | cursor encoding and decoding |
| `src/validation.js` | parameter validation |
| `src/handler.js` | `listItems` / `createApi` |

## Items

`{ id, name, price, updatedAt, category }`. `id` is a unique non-empty string. `name` is a
string or null, `price` a finite number >= 0 or null, `updatedAt` an ISO-8601 UTC timestamp
with milliseconds (`2024-05-01T10:00:00.250Z`, exactly the format of `Date#toISOString`) or
null, and `category` a string. A missing `name`, `price` or `updatedAt` is stored as null.
Only null is "no value": `0` and `""` are ordinary values.

## Parameters

`api.listItems(params)` takes an object (which may itself be omitted); every key is optional:

| Param | Meaning |
|---|---|
| `sort` | `name`, `price` or `updatedAt`, optionally prefixed with `-` for descending. Default `updatedAt`. |
| `limit` | page size, an integer from 1 to 100 inclusive. Default 20. |
| `cursor` | a `nextCursor` or `prevCursor` from an earlier response; absent, `undefined` or `null` means the first page. |
| `category` | only items whose `category` equals this non-empty string (case-sensitive). |
| `minPrice`, `maxPrice` | only items with `minPrice <= price <= maxPrice`; when either is given, items with a null price are excluded. |

A parameter whose value is `undefined` is the same as an absent parameter.

Validation runs in this order and the first failure throws an `ApiError` with
`code: "INVALID_PARAM"`, `status: 400` and exactly this `message`:

1. a key not in the table: `unknown parameter: <key>`
2. bad `sort`: `sort must be one of name, price, updatedAt, optionally prefixed with -`
3. bad `limit` (not an integer, < 1, > 100; a numeric string is not an integer): `limit must be an integer between 1 and 100`
4. `category` not a non-empty string: `category must be a non-empty string`
5. `minPrice` / `maxPrice` not a finite number: `minPrice must be a finite number` (resp. `maxPrice ...`)
6. both given and `minPrice > maxPrice`: `minPrice must not exceed maxPrice`
7. `cursor` neither a string nor null/undefined: `cursor must be a string`

Then the cursor is decoded (see below).

## Ordering

Items are ordered by the sort field, then by `id` as the unique tie-breaker:

- `name` compares strings by UTF-16 code units (the order of JavaScript's `<` on strings):
  case-sensitive, no locale rules, so `"B" < "a"` and `"Z" < "é"`.
- `price` compares numerically.
- `updatedAt` compares chronologically, to the millisecond.
- `id` compares by UTF-16 code units.

Ascending (`price`): non-null values ascending, ties by `id` ascending; then all items whose
value is null, by `id` ascending.

Descending (`-price`): non-null values descending, ties by `id` descending; then all items whose
value is null, by `id` descending.

**Null values sort last in both directions.** So descending is *not* the exact reverse of
ascending: only the non-null block and the null block are each reversed.

## Pages and cursors

A response is `{ items, nextCursor, prevCursor }`; `items` is always in the order above. Let
`R` be the items matching the filters, in order, **as the store is at the time of the request**
(every request reads the current store; an item inserted or deleted before the request is
reflected in it).

- No cursor: `items` is the first `limit` items of `R`.
- A `nextCursor` issued with last item `x`: `items` is the first `limit` items of `R` that sort
  strictly after `x`'s position (its sort value and id, as they were when the cursor was issued).
- A `prevCursor` issued with first item `y`: `items` is the last `limit` items of `R` that sort
  strictly before `y`'s position.

Positions are compared by (sort value, id) in the ordering above, so this is keyset
pagination: a cursor remembers the sort key of the boundary item, never an offset, and works
even if that item has since been deleted.

In every response, `nextCursor` is non-null if and only if at least one item of `R` sorts after
the last item of `items`, and `prevCursor` is non-null if and only if at least one item of `R`
sorts before the first item of `items`. When `items` is empty both cursors are null.

Consequences the API guarantees:

- Walking forward with `nextCursor` from the first page, with any `limit`, yields every item of
  `R` exactly once, in order.
- Walking backward with `prevCursor` from the last page yields the same pages in reverse order.
- Inserting or deleting items between requests never makes the walk skip or repeat an item that
  was in the store (unchanged) for the whole walk; inserted items appear if they sort after the
  cursor position.
- `limit` may differ between requests of the same walk.

### Cursor binding and errors

A cursor is bound to the `sort` and to all filters (`category`, `minPrice`, `maxPrice`) of the
request that issued it. Using it with a different `sort` or with any different filter value
(including adding or removing a filter) throws `ApiError` with `code: "CURSOR_MISMATCH"` and
message `cursor does not match the current sort and filters`. Only `limit` may change.

A string that is not a cursor issued by this API (not base64url, not JSON, or not the cursor
shape) throws `ApiError` with `code: "INVALID_CURSOR"` and message `invalid cursor`.

Cursor format (internal; clients treat cursors as opaque): base64url of the JSON
`{ "v": 1, "q": <query fingerprint>, "d": "next" | "prev", "k": { "value": <sort value>, "id": <id> } }`.
A cursor must carry the boundary item's sort value exactly (timestamps to the millisecond).
