/**
 * Retry an asynchronous task with exponential backoff on an injectable scheduler.
 * See README.md ("retry").
 */
import { once } from "./once.js";
import { realScheduler } from "./scheduler.js";

/** Delay before attempt `attempt + 1`: baseDelay * factor^(attempt - 1), capped at maxDelay. */
export function backoffDelay(attempt, { baseDelay, factor, maxDelay }) {
  return Math.min(maxDelay, baseDelay * factor ** (attempt - 1));
}

/**
 * Calls `task(attempt, cb)` with attempt = 1, 2, ... until it succeeds or `attempts` attempts
 * have failed, or `shouldRetry(error, attempt)` returns false. Between attempts it waits
 * backoffDelay(attempt) on the scheduler. On final failure the last error gets an `attempts`
 * property (the number of attempts made) and is passed to `cb`.
 */
export function retry(task, options, cb) {
  const {
    attempts = 3,
    baseDelay = 100,
    factor = 2,
    maxDelay = 10_000,
    scheduler = realScheduler,
    shouldRetry = () => true,
  } = options ?? {};
  if (!Number.isInteger(attempts) || attempts < 1) {
    process.nextTick(cb, new RangeError("attempts must be a positive integer"));
    return;
  }

  const attempt = (n) => {
    const settle = once((error, value) => {
      if (!error) {
        cb(null, value);
        return;
      }
      if (n >= attempts || !shouldRetry(error, n)) {
        error.attempts = n;
        cb(error);
        return;
      }
      scheduler.setTimeout(() => attempt(n + 1), backoffDelay(n, { baseDelay, factor, maxDelay }));
    });
    try {
      task(n, settle);
    } catch (error) {
      settle(error);
    }
  };

  attempt(1);
}
