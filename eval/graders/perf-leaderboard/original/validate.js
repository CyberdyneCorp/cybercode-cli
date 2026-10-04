/** Argument checks shared by the leaderboard and the service. Messages are part of the contract. */

export function checkPlayer(player) {
  if (typeof player !== "string" || player.length === 0) {
    throw new TypeError("player must be a non-empty string");
  }
}

export function checkScore(score) {
  if (!Number.isSafeInteger(score)) {
    throw new TypeError("score must be a safe integer");
  }
}

export function checkCount(name, value) {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new RangeError(`${name} must be a non-negative integer`);
  }
}

export function checkBound(name, value) {
  if (typeof value !== "number" || Number.isNaN(value)) {
    throw new TypeError(`${name} must be a number`);
  }
}

export function checkPercent(p) {
  if (typeof p !== "number" || !(p > 0 && p <= 100)) {
    throw new RangeError("p must be a number in (0, 100]");
  }
}
