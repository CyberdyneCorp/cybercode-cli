/**
 * The record pipeline: read every JSON Lines file under an input prefix, transform each
 * record, write the results under an output prefix. See README.md ("runPipeline").
 */
import { openStore } from "./fileStore.js";
import { parseLines, serializeLines } from "./jsonl.js";
import { mapLimit } from "./mapLimit.js";
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

/** Run `operation()` under retry with the pipeline's retry options (transient errors only). */
function withRetry(options, operation) {
  return retry(() => operation(), { ...options.retry, shouldRetry: isTransient });
}

/** Append one line per malformed input line to the error log (nothing when there are none). */
async function logMalformed(store, options, key, malformed) {
  if (malformed.length === 0) return;
  const lines = malformed.map((bad) => `${key}:${bad.line}: ${bad.text}\n`).join("");
  await withRetry(options, () => store.append(options.errorLog, lines));
}

/** Move a processed input under the archive prefix, when archiving is enabled. */
async function archiveInput(store, options, key) {
  if (options.archive === null) return;
  const destination = options.archive + key.slice(options.input.length);
  await withRetry(options, () => moveFile(store, key, destination));
}

/** Read an input; null when it has vanished (ENOENT). */
async function readInput(store, options, key) {
  try {
    return await withRetry(options, () => store.read(key));
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

/** Process one input file and return its outcome. */
async function processFile(store, options, key) {
  const target = options.output + key.slice(options.input.length);
  if (await store.exists(target)) return { key, status: "skipped" };
  const text = await readInput(store, options, key);
  if (text === null) return { key, status: "missing" };

  const { records, malformed } = parseLines(text);
  // Records of one file are transformed one at a time, in order.
  const outputs = await mapLimit(records, 1, (record) => options.transform(record));
  const kept = outputs.filter((record) => record !== null);
  await withRetry(options, () => store.write(target, serializeLines(kept)));
  await logMalformed(store, options, key, malformed);
  await archiveInput(store, options, key);
  return { key, status: "done", records: kept.length, dropped: outputs.length - kept.length, malformed: malformed.length };
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

/** Everything between opening and closing the store. */
async function processAll(store, settings) {
  const keys = await store.list(settings.input);
  const inputs = keys.filter((key) => key.endsWith(".jsonl"));
  const outcomes = await mapLimit(inputs, settings.concurrency, (key) => processFile(store, settings, key));
  const summary = summarize(outcomes);
  await withRetry(settings, () => writeJson(store, `${settings.output}_summary.json`, summary));
  return summary;
}

/**
 * Run the pipeline. Options: { store (openStore options), transform(record), input, output,
 * errorLog, archive, concurrency, retry (retry options) }. Resolves with the summary.
 * The store is closed exactly once, whether the run succeeds or fails.
 */
export async function runPipeline(options) {
  const settings = { ...DEFAULTS, ...options };
  if (typeof settings.transform !== "function") throw new TypeError("transform must be a function");
  const store = await openStore(settings.store);
  let summary;
  try {
    summary = await processAll(store, settings);
  } catch (error) {
    await store.close().catch(() => {});
    throw error;
  }
  await store.close();
  return summary;
}
