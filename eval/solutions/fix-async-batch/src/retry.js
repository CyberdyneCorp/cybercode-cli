const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Call `fn(attempt)` until it succeeds, at most `attempts` times. See README.md. */
export async function retry(fn, { attempts = 3, delayMs = 0 } = {}) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    try {
      return await fn(attempt);
    } catch (error) {
      lastError = error;
      if (attempt < attempts) await sleep(delayMs);
    }
  }
  throw lastError;
}
