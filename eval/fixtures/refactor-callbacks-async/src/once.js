/** Wrap a callback so that only its first invocation has any effect. */
export function once(cb) {
  let called = false;
  return (...args) => {
    if (called) return;
    called = true;
    cb(...args);
  };
}
