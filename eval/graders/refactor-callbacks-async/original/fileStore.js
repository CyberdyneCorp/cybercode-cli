/**
 * A simulated key/value file store with asynchronous I/O, used by the pipeline and by tests.
 *
 * The "disk" is the `files` Map passed to openStore; it is read and modified in place. Every
 * operation is validated when it is called, then waits `latency(op, path)` ms on the scheduler;
 * when that delay elapses the `fault(op, path)` hook is consulted and, unless it returns an
 * error, the operation takes effect. See README.md ("File store").
 */
import { realScheduler } from "./scheduler.js";

/** An Error with a `code` property, like Node's fs errors. */
export function storeError(code, message) {
  const error = new Error(message);
  error.code = code;
  return error;
}

const noFault = () => null;
const defaultLatency = () => 1;

function checkPath(path) {
  if (typeof path !== "string" || path.length === 0) return new TypeError("path must be a non-empty string");
  return null;
}

class FileStore {
  #files;
  #scheduler;
  #latency;
  #fault;
  #state = "open"; // "open" | "closing" | "closed"

  constructor({ files, scheduler, latency, fault }) {
    this.#files = files;
    this.#scheduler = scheduler;
    this.#latency = latency;
    this.#fault = fault;
  }

  /** True once close() has been called (even before it completes). */
  get closed() {
    return this.#state !== "open";
  }

  read(path, cb) {
    this.#run("read", path, cb, () => {
      if (!this.#files.has(path)) throw storeError("ENOENT", `no such file: ${path}`);
      return this.#files.get(path);
    });
  }

  write(path, data, cb) {
    if (typeof data !== "string") {
      process.nextTick(cb, new TypeError("data must be a string"));
      return;
    }
    this.#run("write", path, cb, () => {
      this.#files.set(path, data);
    });
  }

  append(path, data, cb) {
    if (typeof data !== "string") {
      process.nextTick(cb, new TypeError("data must be a string"));
      return;
    }
    this.#run("append", path, cb, () => {
      this.#files.set(path, (this.#files.get(path) ?? "") + data);
    });
  }

  exists(path, cb) {
    this.#run("exists", path, cb, () => this.#files.has(path));
  }

  remove(path, cb) {
    this.#run("remove", path, cb, () => {
      if (!this.#files.delete(path)) throw storeError("ENOENT", `no such file: ${path}`);
    });
  }

  /** Paths starting with `prefix` (which may be ""), sorted by UTF-16 code units. */
  list(prefix, cb) {
    if (typeof prefix !== "string") {
      process.nextTick(cb, new TypeError("prefix must be a string"));
      return;
    }
    this.#run("list", prefix, cb, () => [...this.#files.keys()].filter((key) => key.startsWith(prefix)).sort());
  }

  close(cb) {
    if (this.#state !== "open") {
      process.nextTick(cb, storeError("EALREADYCLOSED", "store already closed"));
      return;
    }
    this.#state = "closing";
    const ms = this.#latency("close", "");
    this.#scheduler.setTimeout(() => {
      this.#state = "closed";
      const error = this.#fault("close", "");
      if (error) cb(error);
      else cb(null);
    }, ms);
  }

  /**
   * Shared shape of every path operation: validate now, wait the latency, then consult the
   * fault hook and apply `effect` (whose return value is the result).
   */
  #run(op, path, cb, effect) {
    const invalid = op === "list" ? null : checkPath(path);
    if (invalid) {
      process.nextTick(cb, invalid);
      return;
    }
    if (this.#state !== "open") {
      process.nextTick(cb, storeError("ESTORECLOSED", "store is closed"));
      return;
    }
    const ms = this.#latency(op, path);
    this.#scheduler.setTimeout(() => {
      const error = this.#fault(op, path);
      if (error) {
        cb(error);
        return;
      }
      let result;
      try {
        result = effect();
      } catch (failure) {
        cb(failure);
        return;
      }
      cb(null, result);
    }, ms);
  }
}

/**
 * Open a store over `files` (a Map from path to string contents).
 * Options: { files = new Map(), scheduler = realScheduler, latency = () => 1, fault = () => null }.
 * Opening waits latency("open", "") and consults fault("open", "") like any operation.
 */
export function openStore(options, cb) {
  const { files = new Map(), scheduler = realScheduler, latency = defaultLatency, fault = noFault } = options ?? {};
  if (!(files instanceof Map)) {
    process.nextTick(cb, new TypeError("files must be a Map"));
    return;
  }
  const ms = latency("open", "");
  scheduler.setTimeout(() => {
    const error = fault("open", "");
    if (error) cb(error);
    else cb(null, new FileStore({ files, scheduler, latency, fault }));
  }, ms);
}
