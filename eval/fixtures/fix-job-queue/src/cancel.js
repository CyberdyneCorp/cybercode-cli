/** Cancellation tokens, modelled on AbortController / AbortSignal. */

export class CancelledError extends Error {
  constructor(reason) {
    super("job cancelled");
    this.name = "CancelledError";
    this.reason = reason;
  }
}

class CancelToken {
  #cancelled = false;
  #reason = undefined;
  #handlers = new Set();

  get cancelled() {
    return this.#cancelled;
  }

  get reason() {
    return this.#reason;
  }

  /** Run `handler(reason)` on cancellation; returns a function that unsubscribes it. */
  onCancel(handler) {
    this.#handlers.add(handler);
    return () => this.#handlers.delete(handler);
  }

  /** @internal used by CancelSource */
  _cancel(reason) {
    if (this.#cancelled) return;
    this.#cancelled = true;
    this.#reason = reason;
    for (const handler of [...this.#handlers]) handler(reason);
    this.#handlers.clear();
  }
}

/** `const source = new CancelSource(); queue.add(task, { token: source.token }); source.cancel("why")` */
export class CancelSource {
  token = new CancelToken();

  cancel(reason = "cancelled") {
    this.token._cancel(reason);
  }
}
