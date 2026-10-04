const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Call `fn(attempt)` until it succeeds, at most `attempts` times. See README.md. */
export async function retry(fn, { attempts = 3, delayMs = 0 } = {}) {
  for (let attempt = 1; attempt <= attempts; attempt++) {
    try {
      return fn(attempt);
    } catch (error) {
      if (attempt === attempts) return undefined;
      await sleep(delayMs);
    }
  }
}
