/**
 * The record pipeline: read every JSON Lines file under an input prefix, transform each
 * record, write the results under an output prefix. See README.md ("runPipeline").
 */
import { openStore } from "./fileStore.js";
import { parseLines, serializeLines } from "./jsonl.js";
import { mapLimit } from "./mapLimit.js";
import { once } from "./once.js";
import { retry } from "./retry.js";
import { moveFile, writeJson } from "./storeUtils.js";

const DEFAULTS = {
  input: "in/",
  output: "out/",
  errorLog: "errors.log",
  archive: null,
  concurrency: 2,
  retry: {},
};

/** Errors worth retrying: everything except a missing file and invalid arguments. */
function isTransient(error) {
  return error.code !== "ENOENT" && !(error instanceof TypeError);
}

/** Run `operation(done)` under retry with the pipeline's retry options (transient errors only). */
function withRetry(options, operation, cb) {
  retry((attempt, done) => operation(done), { ...options.retry, shouldRetry: isTransient }, cb);
}

/** Append one line per malformed input line to the error log (nothing when there are none). */
function logMalformed(store, options, key, malformed, cb) {
  if (malformed.length === 0) {
    process.nextTick(cb, null);
    return;
  }
  const lines = malformed.map((bad) => `${key}:${bad.line}: ${bad.text}\n`).join("");
  withRetry(options, (done) => store.append(options.errorLog, lines, done), cb);
}

/** Move a processed input under the archive prefix, when archiving is enabled. */
function archiveInput(store, options, key, cb) {
  if (options.archive === null) {
    process.nextTick(cb, null);
    return;
  }
  const destination = options.archive + key.slice(options.input.length);
  withRetry(options, (done) => moveFile(store, key, destination, done), cb);
}

/** Process one input file. Calls cb(null, outcome) or cb(error). */
function processFile(store, options, key, cb) {
  const target = options.output + key.slice(options.input.length);
  store.exists(target, (existsError, exists) => {
    if (existsError) {
      cb(existsError);
      return;
    }
    if (exists) {
      cb(null, { key, status: "skipped" });
      return;
    }
    withRetry(options, (done) => store.read(key, done), (readError, text) => {
      if (readError && readError.code === "ENOENT") {
        cb(null, { key, status: "missing" });
        return;
      }
      if (readError) {
        cb(readError);
        return;
      }
      const { records, malformed } = parseLines(text);
      // Records of one file are transformed one at a time, in order.
      mapLimit(records, 1, (record, index, done) => options.transform(record, done), (transformError, outputs) => {
        if (transformError) {
          cb(transformError);
          return;
        }
        const kept = outputs.filter((record) => record !== null);
        withRetry(options, (done) => store.write(target, serializeLines(kept), done), (writeError) => {
          if (writeError) {
            cb(writeError);
            return;
          }
          const outcome = { key, status: "done", records: kept.length, dropped: outputs.length - kept.length, malformed: malformed.length };
          logMalformed(store, options, key, malformed, (appendError) => {
            if (appendError) {
              cb(appendError);
              return;
            }
            archiveInput(store, options, key, (archiveError) => {
              if (archiveError) cb(archiveError);
              else cb(null, outcome);
            });
          });
        });
      });
    });
  });
}

function summarize(outcomes) {
  const summary = { files: outcomes.length, processed: 0, skipped: 0, missing: 0, records: 0, dropped: 0, malformed: 0 };
  for (const outcome of outcomes) {
    if (outcome.status === "skipped") summary.skipped += 1;
    else if (outcome.status === "missing") summary.missing += 1;
    else {
      summary.processed += 1;
      summary.records += outcome.records;
      summary.dropped += outcome.dropped;
      summary.malformed += outcome.malformed;
    }
  }
  return summary;
}

/**
 * Run the pipeline. Options: { store (openStore options), transform(record, cb), input,
 * output, errorLog, archive, concurrency, retry (retry options) }. Calls cb(null, summary) or cb(error).
 * The store is closed exactly once, whether the run succeeds or fails.
 */
export function runPipeline(options, cb) {
  const settings = { ...DEFAULTS, ...options };
  if (typeof settings.transform !== "function") {
    process.nextTick(cb, new TypeError("transform must be a function"));
    return;
  }
  openStore(settings.store, (openError, store) => {
    if (openError) {
      cb(openError);
      return;
    }
    const finish = once((error, summary) => {
      store.close((closeError) => {
        if (error) cb(error);
        else if (closeError) cb(closeError);
        else cb(null, summary);
      });
    });
    store.list(settings.input, (listError, keys) => {
      if (listError) {
        finish(listError);
        return;
      }
      const inputs = keys.filter((key) => key.endsWith(".jsonl"));
      mapLimit(inputs, settings.concurrency, (key, index, done) => processFile(store, settings, key, done), (error, outcomes) => {
        if (error) {
          finish(error);
          return;
        }
        const summary = summarize(outcomes);
        const summaryPath = `${settings.output}_summary.json`;
        withRetry(settings, (done) => writeJson(store, summaryPath, summary, done), (writeError) => {
          finish(writeError, summary);
        });
      });
    });
  });
}
