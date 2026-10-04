/**
 * A simulated key/value file store with asynchronous I/O, used by the pipeline and by tests.
 *
 * The "disk" is the `files` Map passed to openStore; it is read and modified in place. Every
 * operation is validated when it is called, then waits `latency(op, path)` ms on the scheduler;
 * when that delay elapses the `fault(op, path)` hook is consulted and, unless it returns an
 * error, the operation takes effect. See README.md ("File store").
 */
import { realScheduler, sleep } from "./scheduler.js";

/** An Error with a `code` property, like Node's fs errors. */
export function storeError(code, message) {
  const error = new Error(message);
  error.code = code;
  return error;
}

const noFault = () => null;
const defaultLatency = () => 1;

function checkPath(path) {
  if (typeof path !== "string" || path.length === 0) throw new TypeError("path must be a non-empty string");
}

function checkData(data) {
  if (typeof data !== "string") throw new TypeError("data must be a string");
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

  async read(path) {
    checkPath(path);
    await this.#wait("read", path);
    if (!this.#files.has(path)) throw storeError("ENOENT", `no such file: ${path}`);
    return this.#files.get(path);
  }

  async write(path, data) {
    checkData(data);
    checkPath(path);
    await this.#wait("write", path);
    this.#files.set(path, data);
  }

  async append(path, data) {
    checkData(data);
    checkPath(path);
    await this.#wait("append", path);
    this.#files.set(path, (this.#files.get(path) ?? "") + data);
  }

  async exists(path) {
    checkPath(path);
    await this.#wait("exists", path);
    return this.#files.has(path);
  }

  async remove(path) {
    checkPath(path);
    await this.#wait("remove", path);
    if (!this.#files.delete(path)) throw storeError("ENOENT", `no such file: ${path}`);
  }

  /** Paths starting with `prefix` (which may be ""), sorted by UTF-16 code units. */
  async list(prefix) {
    if (typeof prefix !== "string") throw new TypeError("prefix must be a string");
    await this.#wait("list", prefix);
    return [...this.#files.keys()].filter((key) => key.startsWith(prefix)).sort();
  }

  async close() {
    if (this.#state !== "open") throw storeError("EALREADYCLOSED", "store already closed");
    this.#state = "closing";
    await sleep(this.#scheduler, this.#latency("close", ""));
    this.#state = "closed";
    const error = this.#fault("close", "");
    if (error) throw error;
  }

  /**
   * Shared timeline of every path operation after argument checks: fail if closed, wait the
   * latency, then throw the fault hook's error if it returns one.
   */
  async #wait(op, path) {
    if (this.#state !== "open") throw storeError("ESTORECLOSED", "store is closed");
    await sleep(this.#scheduler, this.#latency(op, path));
    const error = this.#fault(op, path);
    if (error) throw error;
  }
}

/**
 * Open a store over `files` (a Map from path to string contents).
 * Options: { files = new Map(), scheduler = realScheduler, latency = () => 1, fault = () => null }.
 * Opening waits latency("open", "") and consults fault("open", "") like any operation.
 */
export async function openStore(options) {
  const { files = new Map(), scheduler = realScheduler, latency = defaultLatency, fault = noFault } = options ?? {};
  if (!(files instanceof Map)) throw new TypeError("files must be a Map");
  await sleep(scheduler, latency("open", ""));
  const error = fault("open", "");
  if (error) throw error;
  return new FileStore({ files, scheduler, latency, fault });
}
