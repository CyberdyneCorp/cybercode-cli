/** Multi-step helpers built on the FileStore primitives. */

/** Write `value` as JSON followed by "\n". Calls cb(null) or cb(error). */
export function writeJson(store, path, value, cb) {
  store.write(path, `${JSON.stringify(value)}\n`, (error) => cb(error ?? null));
}

/** Read and parse a JSON file. A parse failure is reported as a SyntaxError. */
export function readJson(store, path, cb) {
  store.read(path, (error, text) => {
    if (error) {
      cb(error);
      return;
    }
    let value;
    try {
      value = JSON.parse(text);
    } catch (parseError) {
      cb(parseError);
      return;
    }
    cb(null, value);
  });
}

/** Copy `from` to `to` (read, then write). Calls cb(null) or cb(error). */
export function copyFile(store, from, to, cb) {
  store.read(from, (readError, data) => {
    if (readError) {
      cb(readError);
      return;
    }
    store.write(to, data, (writeError) => cb(writeError ?? null));
  });
}

/** Move `from` to `to`: copy, then remove the source. Calls cb(null) or cb(error). */
export function moveFile(store, from, to, cb) {
  copyFile(store, from, to, (copyError) => {
    if (copyError) {
      cb(copyError);
      return;
    }
    store.remove(from, (removeError) => cb(removeError ?? null));
  });
}
