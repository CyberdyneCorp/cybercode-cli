/**
 * Binary max-heap of jobs keyed by priority. `pop()` returns the highest-priority job.
 */
export class PriorityQueue {
  #heap = [];

  get length() {
    return this.#heap.length;
  }

  push(item) {
    this.#heap.push(item);
    this.#up(this.#heap.length - 1);
  }

  pop() {
    const heap = this.#heap;
    if (heap.length === 0) return undefined;
    const top = heap[0];
    const last = heap.pop();
    if (heap.length > 0) {
      heap[0] = last;
      this.#down(0);
    }
    return top;
  }

  /** Remove `item` if present; returns whether it was. */
  remove(item) {
    const index = this.#heap.indexOf(item);
    if (index === -1) return false;
    const last = this.#heap.pop();
    if (index < this.#heap.length) {
      this.#heap[index] = last;
      this.#down(index);
      this.#up(index);
    }
    return true;
  }

  #before(a, b) {
    return a.priority > b.priority;
  }

  #up(index) {
    const heap = this.#heap;
    while (index > 0) {
      const parent = (index - 1) >> 1;
      if (!this.#before(heap[index], heap[parent])) break;
      [heap[index], heap[parent]] = [heap[parent], heap[index]];
      index = parent;
    }
  }

  #down(index) {
    const heap = this.#heap;
    for (;;) {
      const left = 2 * index + 1;
      const right = left + 1;
      let best = index;
      if (left < heap.length && this.#before(heap[left], heap[best])) best = left;
      if (right < heap.length && this.#before(heap[right], heap[best])) best = right;
      if (best === index) return;
      [heap[index], heap[best]] = [heap[best], heap[index]];
      index = best;
    }
  }
}
