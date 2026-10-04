# tinyindex

A small full-text indexer for directories of `.txt` and `.md` files (Python 3.10+, standard
library only).

```sh
python3 -m indexer build notes/ --index notes.json      # index a directory
python3 -m indexer search '"release notes" OR changelog' --index notes.json
python3 -m indexer stats --index notes.json
python3 -m unittest                                      # run the tests
```

## Modules

| Module | Responsibility |
|---|---|
| `indexer/config.py` | `IndexConfig(stopwords, stemming, ignore)`, `FORMAT_VERSION` |
| `indexer/walker.py` | which files are indexed; ignore rules |
| `indexer/tokenizer.py` | words, positions, stopwords, stemming |
| `indexer/index.py` | `Document` and the in-memory `InvertedIndex` |
| `indexer/storage.py` | the on-disk format: `encode`, `save_index`, `load_index` |
| `indexer/query.py` | the query language: `search`, `search_file` |
| `indexer/build.py` | `build_index(root, index_path, config=None)` |
| `indexer/cli.py` | the `python3 -m indexer` command line |

## What is indexed

A file under the root is indexed when it is a regular file (symlinks are neither followed
nor indexed) whose name ends in `.txt` or `.md`, no component of its relative path starts
with `.`, and no ignore rule matches it.

The **ignore rules** are `IndexConfig.ignore` followed by the lines of `<root>/.indexignore`
if that file exists (lines stripped; blank lines and lines starting with `#` skipped). Rules
are case-sensitive `fnmatch` globs: a rule containing `/` is matched against the whole
relative POSIX path (`docs/drafts/*`), any other rule against each path component (`build`,
`*.tmp.txt`). A directory matching a rule is skipped entirely. `.indexignore` is itself never
indexed (it starts with `.`).

## Tokenization

Text is decoded as UTF-8 (invalid bytes replaced), case-folded and split into words (runs of
letters and digits). Each word's position is its 0-based ordinal among all words. Stopwords
(`IndexConfig.stopwords`, compared case-folded) are dropped but keep their positions; with
`IndexConfig.stemming` (default on) the remaining words go through `tokenizer.stem`.

## Index file format

`FORMAT_VERSION` is 2. The index file is one JSON object:

```json
{"config":{"ignore":["build"],"stemming":true,"stopwords":["a","the"]},
 "docs":[{"length":2,"mtime_ns":1700000000000000000,"path":"a.txt","sha256":"…","size":14},
         {"length":3,"mtime_ns":1700000005000000000,"path":"b/c.md","sha256":"…","size":17}],
 "format":"tinyindex",
 "postings":{"fox":[[0,[2]]],"quick":[[0,[1]],[1,[0,2]]],"slow":[[1,[1]]]},
 "stats":{"documents":2,"tokens":5},
 "version":2}
```

- `config`: the effective ignore rules (in order), the stemming flag and the sorted stopwords
  the index was built with.
- `docs`: one entry per indexed file, **sorted by path** (plain string order of the relative
  POSIX path); a document's id is its 0-based position in this list. `size` and `mtime_ns`
  are the file's `st_size` and `st_mtime_ns`, `sha256` the hex digest of its raw bytes and
  `length` its number of indexed tokens.
- `postings`: every term that occurs in at least one document, mapped to `[doc_id,
  positions]` pairs in ascending doc_id order, positions ascending.
- `stats`: the number of documents and the sum of their lengths.

The serialization is **canonical**: UTF-8, `json.dumps(data, sort_keys=True,
ensure_ascii=False, separators=(",", ":"))` plus one `"\n"`, so the same index content
always produces the same bytes (`storage.encode`). Files are written atomically.

## Queries

`search(index, query, config)` / `search_file(index_path, query)` return matching paths in
ascending order. Words next to each other (or joined by `AND`) must all match, `OR` gives
alternatives, parentheses group, and `"quoted phrases"` must occur at consecutive positions
(stopword gaps included). See `indexer/query.py` for the grammar.

## Incremental indexing (planned)

Rebuilding a large tree from scratch on every change is slow. `docs/INCREMENTAL.md`
specifies how `build_index` is to reuse the previous on-disk index and what it reports.
