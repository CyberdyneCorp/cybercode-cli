# kv.sh

A tiny file-backed key-value store for shell scripts. It is a single bash script that must use
standard POSIX tools only (no GNU-only flags, no `sed -i`) and behave identically with macOS bash
3.2 + BSD tools and with GNU/Linux. This README is the authoritative contract.

```
kv.sh [-d DIR] set KEY VALUE
kv.sh [-d DIR] get KEY
kv.sh [-d DIR] del KEY
kv.sh [-d DIR] list [PREFIX]
kv.sh [-d DIR] export [--json]
kv.sh [-d DIR] import FILE
kv.sh [-d DIR] incr KEY [N]
```

Results must not depend on the caller's locale, current directory (except for the default store
location) or shell options.

## Keys and values

- A **key** is a non-empty byte string without a newline byte (it may contain spaces, tabs, `/`,
  `*`, `?`, `[`, `%`, `\`, quotes, a leading `-`, `.` or `..`, and non-ASCII/UTF-8 bytes). An
  empty key or a key containing a newline is invalid. Keys longer than 80 bytes need not be
  supported.
- A **value** is any byte string (possibly empty) without NUL bytes: newlines (including leading
  and trailing ones), tabs, carriage returns, backslashes, `%`, text such as `-n`, `-e`, `%s` or
  `*` are all stored and returned byte for byte.
- Command arguments after the command name are always taken literally: `kv.sh set -n -e` stores
  the value `-e` under the key `-n`, and `kv.sh get --` reads the key `--`.

## Storage layout

The store directory is `-d DIR` if given (the last one wins), else `$KV_DIR` if that is set and
non-empty, else `.kv` in the current directory. The value of key K is stored as the file
`DIR/ENC(K)`, whose content is exactly the value bytes (no added newline).

`ENC(K)` maps every byte of K, in order:

- lowercase `a`-`z`, `0`-`9`, `_` and `-` are kept as is;
- `.` is kept, except when it is the first byte of K;
- every other byte (including uppercase `A`-`Z`, a leading `.`, `%`, `/`, space and every byte
  >= 0x80) becomes `%` followed by its value as two **uppercase** hex digits.

Examples: `user/1` → `user%2F1`; `.env` → `%2Eenv`; `..` → `%2E.`; `a b*` → `a%20b%2A`;
`v1.2` → `v1.2`; `Key` → `%4Bey`; `100%` → `100%25`; `é` (UTF-8 `C3 A9`) → `%C3%A9`;
`-n` → `-n`. (Encoding uppercase letters keeps keys that differ only in case apart on
case-insensitive file systems such as the macOS default.)

So key files never start with `.`: names starting with `.` in DIR are reserved for the lock and
temporary files and are never keys. Commands that write (`set`, `del`, `import`, `incr`) create
DIR, including missing parents, when it does not exist; commands that only read (`get`, `list`,
`export`) treat a missing DIR as an empty store and do not create it. A value is replaced
atomically: it is written to a temporary file in DIR whose name starts with `.` and then renamed
over `DIR/ENC(K)`, so a concurrent reader sees the old or the new value, never a partial one.

## Commands

- `set KEY VALUE` — store VALUE under KEY (replacing any previous value). Prints nothing.
- `get KEY` — print the value of KEY exactly, with nothing added (no trailing newline).
- `del KEY` — remove KEY. Prints nothing.
- `list [PREFIX]` — print the keys that start with PREFIX (a plain byte prefix: `*`, `?`, `[`
  and `\` in PREFIX have no special meaning; an empty or omitted PREFIX matches every key),
  sorted ascending by bytes (the order of `LC_ALL=C sort`), each followed by a newline.
- `export` — print every key and value, sorted by key as for `list`, one line per key:
  `ESC(KEY)`, a TAB, `ESC(VALUE)`, a newline. `ESC` replaces `\` with `\\`, TAB with `\t` and
  newline with `\n` (backslash followed by the letter); every other byte is unchanged.
- `export --json` — print one JSON object on a single line followed by a newline:
  `{"KEY1":"VALUE1","KEY2":"VALUE2"}` with keys sorted as for `list`, no spaces, `{}` for an
  empty store. Inside the strings: `"` becomes `\"`, `\` becomes `\\`, newline `\n`, TAB `\t`,
  carriage return `\r`; every other byte 0x01-0x1F and 0x7F becomes `\u00XX` with **lowercase**
  hex digits (ESC is `\u001b`); every other byte, including `/` and bytes >= 0x80, is unchanged.
- `import FILE` — read lines in the `export` format from FILE (`-` means standard input) and
  set each key to its value; keys not mentioned are left alone; when a key appears more than once
  the last line wins. Lines are separated by newlines; the last line may lack its newline; empty
  lines are skipped; a carriage return is an ordinary byte. Every other line must contain exactly
  one TAB byte, which separates `ESC(KEY)` from `ESC(VALUE)`. Unescaping turns `\\` into `\`,
  `\t` into TAB and `\n` into newline; a `\` followed by any other byte, or at the end of the key
  or value, is a bad escape. Import is all-or-nothing: the whole input is checked first and on
  any error nothing is changed. Prints nothing on success.
- `incr KEY [N]` — add N (default 1) to the integer value of KEY (a missing key counts as 0),
  store the result and print it followed by a newline. Integers here are an optional `-` followed
  by 1 to 18 ASCII digits, nothing else (no `+`, no spaces, no trailing newline); leading zeros
  are allowed and the number is always decimal (`007` is 7, `-08` is -8). The result is stored
  and printed in canonical form: no leading zeros, `0` rather than `-0`, no trailing newline in
  the stored value.

`export` output fed to `import` on an empty store recreates the same keys and values.

## Errors

Every error prints exactly one line `kv.sh: MESSAGE` to stderr, prints nothing to stdout,
leaves the store unchanged and exits with the status shown. Problems are checked in this order:
global options, command name, number of arguments, key validity, then the rest.

| Situation | Message | Exit |
|---|---|---|
| an argument before the command starts with `-` and is not `-d` | `unknown option: ARG` | 2 |
| `-d` without a following argument, or with an empty one | `missing value for -d` | 2 |
| no command | `missing command` | 2 |
| unknown command | `unknown command: CMD` | 2 |
| wrong number of arguments (or `export` with an argument other than `--json`) | `usage: kv.sh [-d DIR] SYNOPSIS` with the command's synopsis line from the top of this file, e.g. `usage: kv.sh [-d DIR] incr KEY [N]` | 2 |
| invalid key (empty or containing a newline) | `invalid key` | 2 |
| `incr` with an N that is not an integer as defined above | `invalid increment: N` | 2 |
| `get`/`del` of a missing key | `no such key: KEY` | 1 |
| `incr` on a value that is not an integer as defined above | `not an integer: KEY` | 1 |
| `incr` whose result is not between -999999999999999999 and 999999999999999999 | `integer overflow: KEY` | 1 |
| `import` of a FILE that does not exist, is not a regular file or cannot be read | `cannot read FILE` | 1 |
| `import` line that does not contain exactly one TAB | `import: line L: expected one tab` | 1 |
| `import` line with a bad escape (in the key or the value) | `import: line L: bad escape` | 1 |
| `import` line whose unescaped key is invalid | `import: line L: invalid key` | 1 |
| the store stays locked for longer than the lock timeout | `store is locked` | 3 |

KEY in messages is the key exactly as given. For `import`, L is the 1-based line number of the
first bad line (empty lines count), and for one line the checks run in the order of the table.

## Locking

Write commands (`set`, `del`, `import`, `incr`) hold a lock for their whole read-modify-write,
so concurrent writers never lose updates (e.g. 20 concurrent `incr` of one key from 0 always end
at 20). The lock is the directory `DIR/.lock`, taken with `mkdir` (atomic), and released (removed)
before the command exits, on success and on every error. Right after taking it the holder writes
its PID in decimal followed by a newline to `DIR/.lock/pid`.

When `mkdir` fails because the lock exists:

- if `DIR/.lock/pid` contains a PID of a process that no longer exists, the lock is stale: remove
  it and try again immediately. Removing a stale lock must itself be race-free: a process must
  never remove a lock that another live process has taken in the meantime (for example after
  the dead PID was read, the old lock was released and a new holder took it);
- otherwise (live holder, or a pid file not written yet) wait about 0.1 s and try again, for at
  most `KV_LOCK_TIMEOUT` seconds (a positive integer; default 10 when unset or invalid); then
  fail with `store is locked` (exit 3).

Read commands never take the lock and work while it is held.

## Known problems

Users report that values lose their trailing newlines, keys with spaces or `/` break the store,
`list` shows garbage, `export --json` produces invalid JSON and concurrent `incr` calls lose
updates. Run the tests with `python3 -m unittest`.
