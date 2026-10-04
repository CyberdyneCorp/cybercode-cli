# Incremental indexing: specification

This is the contract for incremental builds. The on-disk format (README.md, "Index file
format") and `FORMAT_VERSION` (2) do not change.

## API

```python
build_index(root, index_path, config=None, incremental=False) -> IndexStats
```

`IndexStats` is importable from `indexer` (and `indexer.build`). It has the int attributes
`files_parsed`, `files_reused`, `files_removed` and the bool attribute `full_rebuild`.
`config=None` means `IndexConfig()`. Every build, incremental or not, writes the index to
`index_path` (atomically, as today).

## The invariant

After any build, the bytes of `index_path` are identical to what a full build
(`incremental=False`) of the same tree with the same config would write at that moment, and
therefore every query returns the same results. This holds as long as no file's content
changes while both its `st_size` and its `st_mtime_ns` stay the same (such a file is
deliberately trusted to be unchanged; see below).

In particular an incremental build must produce exactly what the format requires: documents
sorted by path with ids equal to their position, postings of removed or changed documents
dropped, no term left with an empty postings list, postings in ascending doc_id order and
`stats` recomputed.

## Full builds

With `incremental=False` every indexable file is parsed (stat, read, hashed, tokenized).
The stats are `files_parsed` = number of documents in the new index, `files_reused` = 0,
`files_removed` = 0, `full_rebuild` = True.

## Incremental builds

With `incremental=True` the previous index is the file currently at `index_path`. The build
falls back to a full build, with exactly the full-build stats above, when any of these holds:

1. `index_path` does not exist or `load_index` cannot load it (`IndexFormatError`: unreadable,
   not JSON, not a tinyindex file, a `version` other than `FORMAT_VERSION`, malformed);
2. the recorded `stemming` flag differs from `config.stemming`, or the recorded stopwords
   differ from `sorted(config.stopwords)`;
3. the recorded ignore rules differ from the current effective ignore rules (`config.ignore`
   followed by the current `.indexignore` lines, as returned by `load_ignore_rules`),
   compared as ordered lists.

Otherwise each file of the current walk is handled by path:

| Case | Action | Counted as |
|---|---|---|
| path in the previous index, `st_size` and `st_mtime_ns` both equal to the recorded values | keep the previous entry; the file is not read | reused |
| path in the previous index, size or mtime_ns differs, sha256 of the current bytes equals the recorded sha256 | keep the previous terms, positions and length without re-tokenizing; record the current `size` and `mtime_ns` | reused |
| path in the previous index with a different sha256, or a path not in the previous index | parse the file | parsed |

Paths of the previous index that are not in the current walk are dropped and counted in
`files_removed`. The stats are `files_parsed` and `files_reused` as counted above (so
`files_parsed + files_reused` is the number of documents in the new index), `files_removed`,
and `full_rebuild` = False.

A rename is a removal plus an addition: the old path counts in `files_removed` and the new
path in `files_parsed`. (An implementation may copy the terms of a previous document with the
same sha256 instead of tokenizing the renamed file again, but the file still counts as
parsed; `files_reused` only counts files kept under the same path.)

## Command line

`python3 -m indexer build ROOT [--index PATH] [--incremental] ...` runs `build_index` with
`incremental` set by the new `--incremental` flag and prints exactly one line:

```
indexed <documents> files into <PATH> (parsed <P>, reused <R>, removed <X>)
```

where `<PATH>` is the `--index` value as given.
