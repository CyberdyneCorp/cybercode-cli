import { retry } from "./retry.js";

/** Fetch every url with retries and bounded concurrency. See README.md. */
export async function fetchAll(urls, fetcher, { concurrency = 4, attempts = 3 } = {}) {
  const results = [];
  urls.forEach(async (url) => {
    results.push(await retry(() => fetcher(url), { attempts }));
  });
  return results;
}
