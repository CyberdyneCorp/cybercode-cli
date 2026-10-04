/** Delay before retrying after failed attempt `attempt` (1-based): baseDelay * 2^(attempt - 1), capped. */
export function backoffDelay(attempt, { baseDelay, maxDelay }) {
  return Math.min(maxDelay, baseDelay * 2 ** (attempt - 1));
}

export const realTimers = {
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (handle) => clearTimeout(handle),
};
