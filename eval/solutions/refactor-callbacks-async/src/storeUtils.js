/** Multi-step helpers built on the FileStore primitives. */

/** Write `value` as JSON followed by "\n". */
export async function writeJson(store, path, value) {
  await store.write(path, `${JSON.stringify(value)}\n`);
}

/** Read and parse a JSON file. A parse failure is reported as a SyntaxError. */
export async function readJson(store, path) {
  return JSON.parse(await store.read(path));
}

/** Copy `from` to `to` (read, then write). */
export async function copyFile(store, from, to) {
  await store.write(to, await store.read(from));
}

/** Move `from` to `to`: copy, then remove the source. */
export async function moveFile(store, from, to) {
  await copyFile(store, from, to);
  await store.remove(from);
}
