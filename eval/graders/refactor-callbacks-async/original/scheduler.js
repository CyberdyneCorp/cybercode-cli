/**
 * Timer abstraction. Everything in this package that waits goes through a scheduler, so tests
 * can inject a fake clock. A scheduler is any object with `setTimeout(fn, ms)`.
 */

export const realScheduler = {
  setTimeout(fn, ms) {
    setTimeout(fn, ms);
  },
};

/** Wait `ms` milliseconds on `scheduler`, then call `cb(null)`. */
export function sleep(scheduler, ms, cb) {
  if (typeof ms !== "number" || !(ms >= 0)) {
    process.nextTick(cb, new RangeError("ms must be a non-negative number"));
    return;
  }
  scheduler.setTimeout(() => cb(null), ms);
}
