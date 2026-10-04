# batch-fetch

Fetches many resources with retries and bounded concurrency (Node 22, ES modules, no
dependencies). The fetcher is injected, so tests use fake async functions.

Contract:

- `retry(fn, { attempts = 3, delayMs = 0 } = {})` (`src/retry.js`) calls `fn(attempt)` with a
  1-based attempt number until the promise it returns resolves, at most `attempts` times, waiting
  `delayMs` milliseconds between attempts. It resolves with the first successful value. When
  every attempt fails it rejects with the error of the **last** attempt.
- `fetchAll(urls, fetcher, { concurrency = 4, attempts = 3 } = {})` (`src/fetchAll.js`) calls
  `fetcher(url)` for every url through `retry` and resolves with the results **in the order of
  `urls`**. Up to `concurrency` fetcher calls are in flight at the same time and a new one starts
  as soon as one settles; never more. If a url still fails after all attempts, `fetchAll` rejects
  with that url's last error. No promise rejection may go unhandled. `fetchAll([], f)` resolves
  with `[]`.

Today `fetchAll` resolves before anything is fetched, failures crash the process with unhandled
rejections, and `retry` gives up silently. Run the tests with `node --test`.
