// Cyber Code Workflow prelude (`workflows` → Sandboxed QuickJS runtime, Host API surface,
// Determinism rules). Evaluated once per run before the module; it receives the native
// host bridge, installs the host API as frozen globals, removes code generation and
// non-deterministic sources, and returns the private hooks the engine drives.
(function install(native, argsJson, ctxJson) {
  "use strict";
  const g = globalThis;
  const { defineProperty, freeze, getPrototypeOf, getOwnPropertyNames } = Object;
  const construct = Reflect.construct;
  const stringify = JSON.stringify;
  const parse = JSON.parse;
  const ErrorCtor = Error;
  const PromiseCtor = Promise;
  const MapCtor = Map;
  const isArray = Array.isArray;
  const hostErrors = new WeakSet();

  // Errors created here carry a `kind` scripts can branch on; only these are trusted as
  // typed diagnostics, so a script cannot forge e.g. `BudgetExceeded`.
  function fail(kind, message) {
    const error = new ErrorCtor(message);
    defineProperty(error, "kind", { value: kind, enumerable: true });
    hostErrors.add(error);
    return error;
  }

  function lock(name, value) {
    defineProperty(g, name, { value, writable: false, enumerable: false, configurable: false });
  }

  function deepFreeze(value) {
    if (value !== null && typeof value === "object") {
      for (const key of getOwnPropertyNames(value)) deepFreeze(value[key]);
      freeze(value);
    }
    return value;
  }

  // Determinism: clocks and randomness throw; values come in through `args`.
  const nondeterministic = function () {
    throw fail("ExecutionFailure", "non-deterministic API; pass values through args");
  };
  const RealDate = Date;
  const SafeDate = function Date(...values) {
    if (new.target === undefined || values.length === 0) nondeterministic();
    return construct(RealDate, values, new.target);
  };
  SafeDate.prototype = RealDate.prototype;
  SafeDate.UTC = RealDate.UTC;
  SafeDate.parse = RealDate.parse;
  SafeDate.now = nondeterministic;
  defineProperty(RealDate.prototype, "constructor", { value: SafeDate, writable: false, configurable: false });
  freeze(SafeDate);
  lock("Date", SafeDate);
  defineProperty(Math, "random", { value: nondeterministic, writable: false, configurable: false });
  freeze(Math);
  lock("performance", freeze({ now: nondeterministic }));

  // No code generation: eval, the Function constructors and their prototype back doors.
  for (const fn of [function () {}, async function () {}, function* () {}, async function* () {}]) {
    defineProperty(getPrototypeOf(fn), "constructor", { value: undefined, writable: false, configurable: false });
  }
  // Also garbage-collection observers (WeakRef, FinalizationRegistry) and shared-memory
  // blocking (SharedArrayBuffer, Atomics.wait), which are non-deterministic or uninterruptible.
  for (const name of [
    "eval", "Function", "WebAssembly", "setTimeout", "setInterval", "clearTimeout", "clearInterval",
    "queueMicrotask", "gc", "print", "console", "navigator",
    "WeakRef", "FinalizationRegistry", "SharedArrayBuffer", "Atomics",
  ]) {
    delete g[name];
  }

  // Host calls: the native bridge queues a request and the engine settles it by id.
  const pending = new MapCtor();
  function call(kind, payload) {
    const id = native.request(kind, stringify(payload));
    // The bridge refuses calls past its bounds (pending calls, queued bytes) at once, so
    // a synchronous loop of calls ends instead of piling up rejected promises.
    if (id < 0) throw fail("ExecutionFailure", native.refusal());
    return new PromiseCtor((resolve, reject) => {
      pending.set(id, { resolve, reject });
    });
  }
  function settle(id, ok, json) {
    const entry = pending.get(id);
    if (entry === undefined) return;
    pending.delete(id);
    const value = parse(json);
    if (ok) entry.resolve(value);
    else entry.reject(fail(value.kind, value.message));
  }

  function agent(prompt, options) {
    return call("agent", { prompt, options: options === undefined ? {} : options });
  }
  function ask(question) {
    return call("ask", { question });
  }
  function checkpoint(name, value) {
    return call("checkpoint", { name, value: value === undefined ? null : value });
  }
  function phase(title) {
    if (typeof title !== "string" || title.trim() === "") {
      throw fail("ExecutionFailure", "phase() expects a non-empty title");
    }
    if (!native.phase(title)) throw fail("ExecutionFailure", native.refusal());
  }
  function log(...values) {
    native.log(stringify(values.map((v) => (v === undefined ? null : v))));
  }

  function limitOf(options) {
    const limit = options && options.concurrency;
    if (limit === undefined) return Infinity;
    if (!Number.isInteger(limit) || limit < 1) {
      throw fail("ExecutionFailure", "concurrency must be a positive integer");
    }
    return limit;
  }

  // Run thunks with at most `concurrency` in flight; results keep input order.
  function parallel(thunks, options) {
    if (!isArray(thunks) || thunks.some((t) => typeof t !== "function")) {
      return PromiseCtor.reject(fail("ExecutionFailure", "parallel() expects an array of functions"));
    }
    const settleAll = Boolean(options && options.settle);
    let limit;
    try {
      limit = limitOf(options);
    } catch (error) {
      return PromiseCtor.reject(error);
    }
    return new PromiseCtor((resolve, reject) => {
      const results = new Array(thunks.length);
      let next = 0;
      let active = 0;
      let failed = false;
      const launch = () => {
        if (failed) return;
        if (next === thunks.length && active === 0) return resolve(results);
        while (active < limit && next < thunks.length) {
          const index = next++;
          active++;
          PromiseCtor.resolve()
            .then(() => thunks[index]())
            .then(
              (value) => { results[index] = settleAll ? { status: "fulfilled", value } : value; },
              (reason) => {
                if (settleAll) results[index] = { status: "rejected", reason };
                else { failed = true; reject(reason); }
              },
            )
            .then(() => { active--; launch(); });
        }
      };
      launch();
    });
  }

  // Each item flows through the stages independently; results keep input order.
  function pipeline(items, ...stages) {
    if (!isArray(items) || stages.some((s) => typeof s !== "function")) {
      return PromiseCtor.reject(fail("ExecutionFailure", "pipeline() expects an array and stage functions"));
    }
    return PromiseCtor.all(
      items.map((item, index) =>
        stages.reduce((previous, stage) => previous.then((value) => stage(value, index)), PromiseCtor.resolve(item)),
      ),
    );
  }

  // Direct read-only tools. The engine checks each call against `meta.tools` and the
  // invocation's allowlist, so a script cannot widen it.
  function directTool(name) {
    return function (input) {
      return call("tool", { tool: name, input: input === undefined ? {} : input });
    };
  }
  const tools = {};
  for (const name of ["read", "glob", "grep"]) tools[name] = freeze(directTool(name));

  lock("agent", freeze(agent));
  lock("tools", freeze(tools));
  lock("ask", freeze(ask));
  lock("checkpoint", freeze(checkpoint));
  lock("phase", freeze(phase));
  lock("log", freeze(log));
  lock("parallel", freeze(parallel));
  lock("pipeline", freeze(pipeline));
  lock("args", deepFreeze(parse(argsJson)));
  // `ctx.budget` is live: each read returns the host's latest snapshot.
  const runCtx = parse(ctxJson);
  defineProperty(runCtx, "budget", {
    get: () => deepFreeze(parse(native.budget())),
    enumerable: true,
    configurable: false,
  });
  lock("ctx", deepFreeze(runCtx));

  function describe(error) {
    const isError = error instanceof ErrorCtor;
    let message;
    try {
      message = isError ? String(error.message) : String(error);
    } catch (_) {
      message = "uncaught exception";
    }
    return stringify({
      host: hostErrors.has(error),
      kind: isError && typeof error.kind === "string" ? error.kind : null,
      name: isError ? String(error.name) : null,
      message,
      stack: isError && typeof error.stack === "string" ? error.stack : "",
    });
  }

  // Wait for module evaluation (top-level code), then run the default export.
  function start(evaluation, namespace) {
    return evaluation
      .then(() => {
        const main = namespace.default;
        return typeof main === "function" ? main() : null;
      })
      .then((value) => {
        const json = stringify(value === undefined ? null : value);
        return json === undefined ? "null" : json;
      });
  }

  return freeze({ settle, describe, start });
})
