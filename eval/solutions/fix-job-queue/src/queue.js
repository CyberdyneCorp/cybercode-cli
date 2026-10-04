/**
 * In-process job queue: priorities, a concurrency limit, retries with exponential backoff,
 * cancellation, pause/resume and drain. See README.md for the contract.
 */
import { backoffDelay, realTimers } from "./backoff.js";
import { CancelledError } from "./cancel.js";
import { Emitter } from "./emitter.js";
import { PriorityQueue } from "./priorityQueue.js";

function checkCount(name, value, { min }) {
  if (!Number.isInteger(value) || value < min) {
    throw new RangeError(`${name} must be an integer >= ${min}`);
  }
}

function checkDelay(name, value) {
  if (typeof value !== "number" || !(value >= 0)) throw new RangeError(`${name} must be a number >= 0`);
}

export class JobQueue extends Emitter {
  #concurrency;
  #retries;
  #baseDelay;
  #maxDelay;
  #timers;
  #queue = new PriorityQueue();
  #running = 0;
  /** Entries waiting for their backoff timer. */
  #retrying = new Set();
  #paused = false;
  #nextId = 1;
  #drainWaiters = [];

  constructor({ concurrency = 1, retries = 0, baseDelay = 100, maxDelay = 10_000, timers = realTimers } = {}) {
    super();
    checkCount("concurrency", concurrency, { min: 1 });
    checkCount("retries", retries, { min: 0 });
    checkDelay("baseDelay", baseDelay);
    checkDelay("maxDelay", maxDelay);
    this.#concurrency = concurrency;
    this.#retries = retries;
    this.#baseDelay = baseDelay;
    this.#maxDelay = maxDelay;
    this.#timers = timers;
  }

  /** Snapshot of the queue's state. */
  get stats() {
    return {
      queued: this.#queue.length,
      running: this.#running,
      retrying: this.#retrying.size,
      paused: this.#paused,
    };
  }

  /**
   * Add a job. `task({ attempt, token })` may return a value or a promise. Returns a promise for
   * the job's result.
   */
  add(task, { priority = 0, retries = this.#retries, token, id } = {}) {
    if (typeof task !== "function") throw new TypeError("task must be a function");
    if (typeof priority !== "number" || !Number.isFinite(priority)) throw new TypeError("priority must be a finite number");
    checkCount("retries", retries, { min: 0 });

    const entry = {
      job: { id: id ?? this.#nextId++, priority, attempt: 0 },
      priority,
      task,
      retries,
      token,
      running: false,
      settled: false,
      timer: null,
      unsubscribe: null,
    };
    const promise = new Promise((resolve, reject) => {
      entry.resolve = resolve;
      entry.reject = reject;
    });

    if (token?.cancelled) {
      this.#settle(entry, "cancel", token.reason);
      return promise;
    }
    if (token) entry.unsubscribe = token.onCancel((reason) => this.#cancel(entry, reason));
    this.#queue.push(entry);
    this.#pump();
    return promise;
  }

  /** Stop starting jobs. Running jobs carry on. */
  pause() {
    this.#paused = true;
  }

  /** Start jobs again, into the slots that are free. */
  resume() {
    if (!this.#paused) return;
    this.#paused = false;
    this.#pump();
  }

  /** Resolves once no job is queued, running or waiting for a retry. */
  drain() {
    if (this.#idle()) return Promise.resolve();
    return new Promise((resolve) => this.#drainWaiters.push(resolve));
  }

  #idle() {
    return this.#queue.length === 0 && this.#running === 0 && this.#retrying.size === 0;
  }

  #checkDrain() {
    if (!this.#idle()) return;
    const waiters = this.#drainWaiters;
    this.#drainWaiters = [];
    for (const resolve of waiters) resolve();
  }

  #pump() {
    while (!this.#paused && this.#running < this.#concurrency && this.#queue.length > 0) {
      this.#start(this.#queue.pop());
    }
  }

  #start(entry) {
    this.#running += 1;
    entry.running = true;
    entry.job.attempt += 1;
    this.emit("start", entry.job);
    let outcome;
    try {
      outcome = Promise.resolve(entry.task({ attempt: entry.job.attempt, token: entry.token }));
    } catch (error) {
      // A synchronous throw is a failed attempt like a rejection: release the slot first.
      this.#release(entry);
      this.#failed(entry, error);
      return;
    }
    outcome.then(
      (value) => {
        this.#release(entry);
        this.#succeeded(entry, value);
      },
      (error) => {
        this.#release(entry);
        this.#failed(entry, error);
      },
    );
  }

  #release(entry) {
    this.#running -= 1;
    entry.running = false;
  }

  #succeeded(entry, value) {
    this.#settle(entry, "success", value);
    this.#pump();
    this.#checkDrain();
  }

  #failed(entry, error) {
    if (entry.token?.cancelled) {
      this.#settle(entry, "cancel", entry.token.reason);
    } else if (entry.job.attempt <= entry.retries) {
      this.#scheduleRetry(entry, error);
    } else {
      this.#settle(entry, "failure", error);
    }
    this.#pump();
    this.#checkDrain();
  }

  #scheduleRetry(entry, error) {
    const delay = backoffDelay(entry.job.attempt, { baseDelay: this.#baseDelay, maxDelay: this.#maxDelay });
    this.#retrying.add(entry);
    this.emit("retry", entry.job, error, delay);
    entry.timer = this.#timers.setTimeout(() => {
      entry.timer = null;
      this.#retrying.delete(entry);
      this.#queue.push(entry);
      this.#pump();
    }, delay);
  }

  /** Token handler: a queued or backing-off job is dropped; a running attempt is left to finish. */
  #cancel(entry, reason) {
    if (entry.settled || entry.running) return;
    if (entry.timer !== null) {
      this.#timers.clearTimeout(entry.timer);
      entry.timer = null;
      this.#retrying.delete(entry);
    } else {
      this.#queue.remove(entry);
    }
    this.#settle(entry, "cancel", reason);
    this.#checkDrain();
  }

  /** Final outcome: emit the event, then settle the job's promise. */
  #settle(entry, outcome, payload) {
    entry.settled = true;
    entry.unsubscribe?.();
    this.emit(outcome, entry.job, payload);
    if (outcome === "success") entry.resolve(payload);
    else if (outcome === "failure") entry.reject(payload);
    else entry.reject(new CancelledError(payload));
  }
}
