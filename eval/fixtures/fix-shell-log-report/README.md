# latency_report.sh

Ops helper that summarizes API access logs per endpoint: request count, error rate and latency
percentiles. It is written in bash with awk helpers in `lib/` and must use standard POSIX tools
only (no gawk extensions, no `sed -i`, no GNU-only flags), behaving identically with macOS bash
3.2 + BSD tools and with GNU/Linux. This README is the authoritative contract.

## Usage

```
latency_report.sh [-n TOP] [--since TS] [--status CLASS] [--] LOG...
```

The script works from any current directory and when its own path (or the path of `lib/`)
contains spaces. It never writes anywhere except stdout, stderr and temporary files it removes.

## Log format

Each line of a log is one request with exactly five fields separated by one or more spaces or
tabs (leading and trailing spaces/tabs are ignored):

```
TIMESTAMP METHOD PATH STATUS LATENCY_MS
2024-05-01T12:00:03Z GET /api/users/42?verbose=1 200 87
```

A line is **valid** when all of these hold, and **malformed** otherwise:

- there are exactly five fields;
- `TIMESTAMP` has the form `YYYY-MM-DDTHH:MM:SSZ`: 4 digits, `-`, 2 digits, `-`, 2 digits, `T`,
  2 digits, `:`, 2 digits, `:`, 2 digits, `Z` (ASCII digits only; ranges are not checked);
- `METHOD` is one or more ASCII uppercase letters `A`-`Z`;
- `PATH` starts with `/`;
- `STATUS` is three ASCII digits whose first digit is `1`-`5` (100-599);
- `LATENCY_MS` is 1 to 9 ASCII digits; its value is the decimal integer (leading zeros allowed:
  `007` is 7), and it is always printed without leading zeros.

Blank lines (empty, or only spaces/tabs) are ignored and are not malformed. Malformed lines are
ignored for the report; if there were any (summed over all files, counted before any
`--since`/`--status` filtering), the script prints exactly one line to stderr:

```
latency_report.sh: skipped N malformed lines
```

(always this wording, even when N is 1) and still exits 0. With no malformed lines stderr is
empty.

## Input files

- Every `LOG` argument is a file path. Paths may contain spaces, glob characters and other
  special characters, and may start with `-` (pass such paths after `--`, or after another LOG,
  see below).
- A path ending in `.gz` is decompressed (`gzip -dc`); any other path is read as plain text.
  Plain and `.gz` files can be mixed freely; the entries of all files are combined into a single
  report. A last line without a trailing newline is still a line.
- Before any output, every LOG is checked in command-line order. If a path does not exist, is not
  a regular file or is not readable, the script prints `latency_report.sh: cannot read PATH` to
  stderr and exits 1; if a `.gz` file cannot be decompressed (not gzip data, truncated, ...), it
  prints `latency_report.sh: cannot decompress PATH` and exits 1. Only the first failing file
  is reported, and nothing at all is written to stdout in these cases.

## Options

Options are recognized only before the first LOG argument. Parsing stops at `--` (which is
consumed) or at the first argument that does not start with `-`; every argument after that is a
LOG path, even if it starts with `-`. When an option is given more than once, the last value wins.

- `-n TOP` — print at most TOP endpoint rows (default 10). TOP is digits only and at least 1
  (`0`, `-1`, `abc`, `2.5`, empty are invalid; leading zeros like `05` are fine).
- `--since TS` — keep only entries whose timestamp is at or after TS. TS must have the exact
  `TIMESTAMP` form above. Because every timestamp is fixed-width, zero-padded UTC, "at or after"
  is a plain string comparison: `TIMESTAMP >= TS`.
- `--status CLASS` — keep only entries whose status is in CLASS, one of `1xx`, `2xx`, `3xx`,
  `4xx`, `5xx` (lowercase `xx`, exactly), i.e. whose status starts with that digit.

The value of an option is always the next argument (`-n5` and `--since=TS` are not supported and
count as unknown options).

### Usage errors

Usage errors print two lines to stderr, nothing to stdout, and exit 2. The first line is one of

```
latency_report.sh: unknown option: ARG
latency_report.sh: missing value for OPTION        (OPTION is -n, --since or --status)
latency_report.sh: invalid -n value: VALUE
latency_report.sh: invalid --since value: VALUE
latency_report.sh: invalid --status value: VALUE
latency_report.sh: no log files given
```

and the second line is always

```
usage: latency_report.sh [-n TOP] [--since TS] [--status CLASS] [--] LOG...
```

An argument is an unknown option when it is seen before the first LOG, starts with `-` and is not
`-n`, `--since`, `--status` or `--` (this includes `-` alone). Arguments are processed left to
right and the first problem found is reported; usage errors are detected before any file is
checked.

## Endpoints and path normalization

Entries are grouped by endpoint: `METHOD`, one space, normalized path (`GET /api/users/:id`).
The path is normalized in this order:

1. Strip the query string and fragment: everything from the first `?` or `#` (whichever comes
   first) to the end.
2. Collapse every run of consecutive `/` into a single `/`.
3. Remove one trailing `/`, unless the path is exactly `/`.
4. Replace whole segments (the text between two `/`, or after the last `/`):
   - a segment made only of ASCII digits becomes `:id`;
   - a segment of the form 8-4-4-4-12 hexadecimal digits separated by `-` (36 characters,
     digits `0-9`, `a-f` and `A-F`, any mix of case) becomes `:uuid`;
   - every other segment is kept byte for byte (case is preserved).

Examples: `/api//users/42/?x=1` → `/api/users/:id`; `/?q=1` → `/`; `//` → `/`;
`/files/0042#top` → `/files/:id`; `/o/3F2504E0-4F89-11D3-9A0C-0305E82C3301/items` →
`/o/:uuid/items`; `/v2/a1b2` → `/v2/a1b2`.

## Report

Output is tab-separated (one `\t` between columns, no padding), one header line and then one row
per endpoint:

```
endpoint	count	error%	p50	p95	p99	max
GET /api/users/:id	3	33.3%	87	120	120	120
```

- `count` — number of entries for the endpoint (after filtering).
- `error%` — the share of entries with a 5xx status, as a percentage with exactly one decimal,
  rounded half up, followed by `%`. Exactly: let `t = floor((2000 * errors + count) /
  (2 * count))` (an integer number of tenths of a percent); print `t / 10` (integer division),
  `.`, `t mod 10`, `%`. So 1 of 16 is `6.3%`, 1 of 3 is `33.3%`, 2 of 3 is `66.7%`, 0 is `0.0%`,
  all is `100.0%`.
- `p50`, `p95`, `p99` — nearest-rank percentiles of the latencies: with the `n` latencies sorted
  ascending (numerically), the p-th percentile is the k-th smallest where
  `k = ceil(p * n / 100)` (1-based). E.g. for n = 10, p95 is the 10th smallest and p50 the 5th;
  for n = 20, p95 is the 19th.
- `max` — the largest latency.

Rows are sorted by `count` descending, then by `endpoint` ascending as plain byte strings (the
order of `LC_ALL=C sort`), whatever the caller's locale. Only the first TOP rows are printed. If
no entries remain (empty logs, everything malformed or filtered out), only the header is printed.
Exit status is 0 whenever a report is printed.

Logs of 50,000 lines must be processed in well under 10 seconds.

## Known problems

On-call reports that the script breaks on log paths with spaces, gets percentiles wrong, does not
group UUID paths consistently, mishandles compressed archives and prints errors to stdout. Run
the tests with `python3 -m unittest`.
