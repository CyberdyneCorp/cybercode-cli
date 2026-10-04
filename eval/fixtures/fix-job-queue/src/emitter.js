/** Minimal synchronous event emitter. */
export class Emitter {
  #listeners = new Map();

  on(event, listener) {
    if (!this.#listeners.has(event)) this.#listeners.set(event, []);
    this.#listeners.get(event).push(listener);
    return this;
  }

  off(event, listener) {
    const listeners = this.#listeners.get(event);
    if (listeners) this.#listeners.set(event, listeners.filter((l) => l !== listener));
    return this;
  }

  once(event, listener) {
    const wrapper = (...args) => {
      this.off(event, wrapper);
      listener(...args);
    };
    return this.on(event, wrapper);
  }

  /** Call every listener registered for `event`, in registration order. */
  emit(event, ...args) {
    for (const listener of [...(this.#listeners.get(event) ?? [])]) listener(...args);
  }
}
