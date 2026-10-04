/**
 * Retry an asynchronous task with exponential backoff on an injectable scheduler.
 * See README.md ("retry").
 */
import { realScheduler, sleep } from "./scheduler.js";

/** Delay before attempt `attempt + 1`: baseDelay * factor^(attempt - 1), capped at maxDelay. */
export function backoffDelay(attempt, { baseDelay, factor, maxDelay }) {
  return Math.min(maxDelay, baseDelay * factor ** (attempt - 1));
}

/**
 * Calls `task(attempt)` with attempt = 1, 2, ... until it resolves or `attempts` attempts have
 * failed, or `shouldRetry(error, attempt)` returns false. Between attempts it waits
 * backoffDelay(attempt) on the scheduler. On final failure the last error gets an `attempts`
 * property (the number of attempts made) and is rethrown.
 */
export async function retry(task, options) {
  const {
    attempts = 3,
    baseDelay = 100,
    factor = 2,
    maxDelay = 10_000,
    scheduler = realScheduler,
    shouldRetry = () => true,
  } = options ?? {};
  if (!Number.isInteger(attempts) || attempts < 1) throw new RangeError("attempts must be a positive integer");

  for (let attempt = 1; ; attempt += 1) {
    try {
      return await task(attempt);
    } catch (error) {
      if (attempt >= attempts || !shouldRetry(error, attempt)) {
        error.attempts = attempt;
        throw error;
      }
    }
    await sleep(scheduler, backoffDelay(attempt, { baseDelay, factor, maxDelay }));
  }
}
