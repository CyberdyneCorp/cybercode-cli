/**
 * Timer abstraction. Everything in this package that waits goes through a scheduler, so tests
 * can inject a fake clock. A scheduler is any object with `setTimeout(fn, ms)`.
 */

export const realScheduler = {
  setTimeout(fn, ms) {
    setTimeout(fn, ms);
  },
};

/** Wait `ms` milliseconds on `scheduler`. */
export async function sleep(scheduler, ms) {
  if (typeof ms !== "number" || !(ms >= 0)) throw new RangeError("ms must be a non-negative number");
  await new Promise((resolve) => {
    scheduler.setTimeout(resolve, ms);
  });
}
