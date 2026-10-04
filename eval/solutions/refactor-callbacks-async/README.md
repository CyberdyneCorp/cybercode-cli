# record-pipeline

Reads JSON Lines files from a (simulated) file store, transforms every record and writes the
results back (Node 22, ES modules, no dependencies). The public API is exported from
`src/index.js`. Tests: `node --test`.

| Module | Contents |
|---|---|
| `src/scheduler.js` | `realScheduler`, `sleep` |
| `src/fileStore.js` | `openStore`, `storeError`, the `FileStore` returned by `openStore` |
| `src/storeUtils.js` | `copyFile`, `moveFile`, `readJson`, `writeJson` |
| `src/mapLimit.js` | `mapLimit` |
| `src/retry.js` | `retry`, `backoffDelay` |
| `src/jsonl.js` | `parseLines`, `serializeLines` (synchronous) |
| `src/pipeline.js` | `runPipeline` |

Every asynchronous function and store method returns a Promise: it resolves with the result
("succeeds") or rejects with the error ("fails"). Argument errors are rejections too, never
synchronous throws. No function takes a completion callback.

## Scheduler

A scheduler is any object with `setTimeout(fn, ms)`; `realScheduler` uses the global timer.
All waiting in this package (store latency, retry backoff, `sleep`) goes through the scheduler
it was given, so a test can inject a fake clock. Nothing else defers work: there are no other
timers, `setImmediate` calls or waiting loops.

- `sleep(scheduler, ms)`: calls `scheduler.setTimeout` once with `ms`, then resolves.
  `ms` that is not a number `>= 0` fails with `RangeError("ms must be a non-negative number")`
  without touching the scheduler.

## File store

`openStore(options)` with `options = { files = new Map(), scheduler = realScheduler,
latency = () => 1, fault = () => null }` yields a `FileStore`. `files` maps paths to string
contents; it is the "disk" and is read and modified in place. Opening calls `latency("open", "")`,
waits that long on the scheduler, then calls `fault("open", "")`: if that returns an Error, opening
fails with that same Error object. `files` that is not a Map fails with
`TypeError("files must be a Map")` before any hook is called.

Every store operation follows the same timeline:

1. When called, it validates its arguments, then the store state, and fails at once (no hooks,
   no scheduler call) on a problem: a path that is not a non-empty string gives
   `TypeError("path must be a non-empty string")`; for `write`/`append`, data that is not a string gives
   `TypeError("data must be a string")` (checked before the path); for `list`, a prefix that is not
   a string gives `TypeError("prefix must be a string")`; after `close()` has been called, any
   operation gives an Error with `code: "ESTORECLOSED"`, message `"store is closed"`.
2. It calls `latency(op, path)` (`list` passes the prefix) and waits that many ms on the scheduler.
3. When the wait is over it calls `fault(op, path)`; a returned Error fails the operation with that
   same object and the operation has no effect. Otherwise the operation takes effect **now**
   (so it sees writes that completed while it was waiting).

| Operation | Result |
|---|---|
| `read(path)` | the contents; Error `code: "ENOENT"`, message `"no such file: <path>"` if missing |
| `write(path, data)` | creates or replaces the file; no result |
| `append(path, data)` | appends, creating the file if missing; no result |
| `exists(path)` | `true` / `false` |
| `remove(path)` | deletes; `ENOENT` error as for `read` if missing |
| `list(prefix)` | paths starting with `prefix` (may be `""`), sorted by UTF-16 code units |
| `close()` | marks the store closed immediately, calls `latency("close", "")`, waits, then calls `fault("close", "")` (a returned Error fails the close, but the store stays closed). Calling `close` again fails at once with code `"EALREADYCLOSED"`, message `"store already closed"` |

`store.closed` (getter) is true once `close` has been called. `storeError(code, message)` makes an
Error with a `code`.

`src/storeUtils.js`: `writeJson(store, path, value)` writes `JSON.stringify(value) + "\n"`;
`readJson(store, path)` reads and `JSON.parse`s (a parse failure is the SyntaxError);
`copyFile(store, from, to)` reads then writes; `moveFile(store, from, to)` copies then
removes the source. Each step starts only after the previous one succeeded; the first error is
the result. None of them has a result value.

## mapLimit

`mapLimit(items, limit, iterator)` calls the async `iterator(item, index)` for every item, in index
order, with at most `limit` calls in flight: it starts `min(limit, items.length)` calls
immediately and starts the next one as soon as one finishes. It succeeds with the results in input
order (`[]` for no items, without calling the iterator). On the first failure it starts nothing
new and fails immediately with that error, without waiting for the calls still in flight; their
later outcomes (successes or errors) are ignored. Arguments are checked in this order: `items`
not an array: `TypeError("items must be an array")`; `limit` not a positive integer:
`RangeError("limit must be a positive integer")`.

## retry

`retry(task, options)` with `options = { attempts = 3, baseDelay = 100, factor = 2,
maxDelay = 10000, scheduler = realScheduler, shouldRetry = () => true }` (omitted options or
an omitted `options` object take these defaults) calls
the async `task(attempt)` with `attempt = 1, 2, ...`. It succeeds with the first successful attempt's
value. After failed attempt `n` it gives up if `n === attempts` (without calling `shouldRetry`)
or else if `shouldRetry(error, n)` returns false; otherwise it calls `scheduler.setTimeout` once with `backoffDelay(n, options)` =
`min(maxDelay, baseDelay * factor ** (n - 1))` and then makes attempt `n + 1`. When it gives up it
sets `error.attempts = n` on the last attempt's error and fails with that same object. A task that
throws synchronously counts as a failed attempt. `attempts` not a positive integer:
`RangeError("attempts must be a positive integer")`, without calling the task.

## runPipeline

`runPipeline(options)` with `options = { store, transform, input = "in/", output = "out/",
errorLog = "errors.log", archive = null, concurrency = 2, retry = {} }`; `store` holds the
`openStore` options and `retry` the retry options (any `shouldRetry` there is replaced, see
below). `transform` not a function fails with `TypeError("transform must be a function")` before
the store is opened.

1. Open the store (an open failure is the pipeline's error; nothing to close).
2. `list(input)`; the inputs are the listed paths ending in `.jsonl`, in listing order.
3. Process the inputs with `mapLimit(inputs, concurrency, ...)`. For input `key` with
   `name = key.slice(input.length)`:
   1. `exists(output + name)`: if it exists the input is **skipped** (a checkpoint).
   2. `read(key)` with retry. If it fails with `ENOENT` the input is **missing** (not an error).
   3. `parseLines` the text. Records are passed to the async `transform(record)` **one at a time, in
      order** (a mapLimit with limit 1); a transform failure fails the input. A transform result of
      `null` drops the record.
   4. `write(output + name, serializeLines(kept records))` with retry.
   5. If there were malformed lines, `append(errorLog, ...)` with retry, one line per malformed
      line: `` `${key}:${lineNumber}: ${lineText}\n` ``.
   6. If `archive` is a string, `moveFile(key, archive + name)` with retry (the whole move is
      retried).
   7. The input is **done** with `{ records: kept, dropped: transform nulls, malformed }`.
4. Write the summary `{ files, processed, skipped, missing, records, dropped, malformed }`
   (counts over the inputs; `processed` counts done inputs, the last three are summed over them)
   with `writeJson(output + "_summary.json", summary)` with retry.
5. Close the store, exactly once, whether the run succeeded or failed at any step after
   opening, and only after that step has failed or the summary was written. The pipeline then
   fails with the step's error if there was one (a close error is then ignored), else with the
   close error if close failed, else succeeds with the summary.

"With retry" means `retry` with the pipeline's retry options and a `shouldRetry` that retries
every error except one with `code: "ENOENT"` or a `TypeError`. `exists` and `list` are not
retried. When a step fails, work already in flight for other inputs is not cancelled: it runs
on (its store calls fail with `ESTORECLOSED` once the store is closed, and those are retried
like any transient error), and its outcome is ignored.
