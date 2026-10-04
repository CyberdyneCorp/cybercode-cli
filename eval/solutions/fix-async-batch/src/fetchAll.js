import { retry } from "./retry.js";

/** Fetch every url with retries and bounded concurrency. See README.md. */
export async function fetchAll(urls, fetcher, { concurrency = 4, attempts = 3 } = {}) {
  const results = new Array(urls.length);
  let next = 0;

  // Each worker takes the next unclaimed index until none are left.
  async function worker() {
    while (next < urls.length) {
      const index = next++;
      results[index] = await retry(() => fetcher(urls[index]), { attempts });
    }
  }

  const workers = Array.from({ length: Math.min(concurrency, urls.length) }, worker);
  await Promise.all(workers);
  return results;
}
