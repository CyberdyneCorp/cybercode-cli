/** An unbounded async queue: producers `push`, one consumer iterates until `end`. */
export class AsyncQueue<T> implements AsyncIterable<T> {
  private items: T[] = [];
  private waiting: ((result: IteratorResult<T>) => void) | undefined;
  private failWaiting: ((err: unknown) => void) | undefined;
  private done = false;
  private error: unknown;

  push(item: T): void {
    if (this.done) return;
    if (this.waiting) this.settle().resolve({ value: item, done: false });
    else this.items.push(item);
  }

  /** Stop the queue; iteration ends after buffered items, rejecting with `error` when given. */
  end(error?: unknown): void {
    if (this.done) return;
    this.done = true;
    this.error = error;
    if (!this.waiting) return;
    const { resolve, reject } = this.settle();
    if (error === undefined) resolve({ value: undefined, done: true });
    else reject(error);
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<T> {
    for (;;) {
      const item = this.items.shift();
      if (item !== undefined) {
        yield item;
        continue;
      }
      if (this.done) {
        if (this.error !== undefined) throw this.error;
        return;
      }
      const next = await new Promise<IteratorResult<T>>((resolve, reject) => {
        this.waiting = resolve;
        this.failWaiting = reject;
      });
      if (next.done) return;
      yield next.value;
    }
  }

  private settle(): { resolve: (result: IteratorResult<T>) => void; reject: (err: unknown) => void } {
    const settled = { resolve: this.waiting!, reject: this.failWaiting! };
    this.waiting = undefined;
    this.failWaiting = undefined;
    return settled;
  }
}
