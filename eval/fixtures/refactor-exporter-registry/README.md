# reportx

Export a tabular dataset as a report file (Python 3.10+, standard library only).

```sh
python3 -m reportx sample.json --format markdown          # to stdout
python3 -m reportx sample.json -f csv,html --output-dir out/  # one file per format
python3 -m reportx sample.json -o report.htm               # format guessed from the suffix
python3 -m reportx --list-formats
python3 -m unittest                                        # tests
```

A dataset is a JSON object `{"title": str, "columns": [str, ...], "rows": [[cell, ...], ...]}`
whose cells are strings, numbers, booleans or null (see `reportx/dataset.py`).

## Formats

| Format | Aliases | Extension | Content-Type | Notes |
|---|---|---|---|---|
| csv | | .csv | text/csv; charset=utf-8 | RFC 4180 quoting, CRLF line endings, `""` for an empty string and nothing for null |
| tsv | | .tsv | text/tab-separated-values; charset=utf-8 | backslash escapes, `\N` for null |
| json | | .json | application/json | one document, indented, UTF-8 |
| ndjson | jsonl | .ndjson | application/x-ndjson | one compact ASCII-only object per row, no trailing newline |
| markdown | md | .md | text/markdown; charset=utf-8 | title heading, right-aligned numeric columns, row count footer; requires a header |
| html | htm | .html | text/html; charset=utf-8 | `<table>` with caption; served inline, all others as attachments |
| xml | | .xml | application/xml | `<report>` of `<row>`/`<field>` elements |

Format names are case-insensitive and may be given by alias. `--no-header` drops the
column-name row of csv, tsv and html, is an error for markdown, and is ignored (with a warning)
for json, ndjson and xml.

## Library

`render(dataset, fmt, header=True)`, `content_type(fmt)`, `extension(fmt)`,
`output_filename(title, fmt)`, `guess_format(path)`, `response_headers(dataset, fmt)`,
`normalize_format(name)`, `parse_format_list(text)`, `export_all(dataset, formats, directory,
header=True)`, `load_dataset(path)` and `dataset_from_dict(data)` are exported from `reportx`.
The code is the reference for their exact output.
