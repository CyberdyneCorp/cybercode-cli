# Binary workspace

Open a regular file in the hex/ASCII workspace without starting inference:

```sh
cyber hex firmware.bin
cyber --cwd /path/to/project hex fixtures/header.bin --offset 0x10 --read-only
cyber hex fixtures/header.bin --session ses_…
```

In an existing terminal Session, use `/hex <project-relative path>` or press `x`
on an explorer file. The Session's read rules apply to every request, including
normal permission prompts. Selecting an existing Session never rebinds its
Location; the direct command refuses a target from another project.

The current implementation is **live viewing and inspection**. Staged editing,
review/save/recovery, the model `binary` tool and snapshot integration remain
unfinished under [issue #94](https://github.com/CyberdyneCorp/cybercode-cli/issues/94).
Both normal and `--read-only` entry currently open a read-only view.

## Navigation and interpretation

- Arrow/Page/Home/End keys move the shared hex/ASCII cursor. Rows adapt to 16, 8
  or 4 bytes. Nonprintable bytes always display as `.`; content never becomes
  terminal escape sequences.
- `g` accepts decimal, `0x` hexadecimal and signed relative offsets. Arithmetic
  and EOF checks use exact integers, including values beyond JavaScript's safe
  integer range where the native filesystem can address them.
- `/` opens literal hex search. Tab changes the encoding to literal UTF-8.
  Results are bounded slices; F3 visits remaining matches, then continues scanning.
  There are no regexes or format parsers.
- `l` changes LE/BE inspector interpretation. Integer, float and bit displays do
  not change bytes or establish that a file format is valid. Missing bytes are
  explicitly unavailable.
- `b` names a bookmark (64 per open document); `n` visits the next bookmark.
  Bookmarks last for that view and do not enter model context.
- `c` explicitly copies the current byte as hex. Esc closes the live view and
  releases its server resource. Switching Session/Location clears cached bytes.

Actions use the `hex` keybinding context; explorer entry uses `explorer_hex` in
`explorer`. The footer follows configured bindings.

A live revision describes an **observed metadata generation**, not an immutable
whole-file digest. Pages carry their own SHA-256. Replacement of a file or parent,
or an observed metadata change, invalidates the document; close and reopen it.
On Windows, retained native directory handles can prevent a parent directory from
being renamed until the view closes.
Live search is best-effort against external writers. Mutation will require a
separately prepared immutable base and exact content evidence.

## Owner API and SDK

The authenticated `fs` group adds:

| Operation | Route |
|---|---|
| Open | `POST /api/v1/files/binary/documents` (`session_id`, `path`, optional `expected_root`) |
| Byte page | `GET …/{id}/bytes?session_id=…&revision=…&offset=0&limit=65536` |
| Literal search | `POST …/{id}/search` (`session_id`, `revision`, `hex`, decimal-string `offset`) |
| Close | `DELETE …/{id}?session_id=…` |

Opaque document IDs do not grant Session or Location authority. Routes retain
owner authentication, Origin policy and `no-store`. Reads recheck Session/Location
and current read permission after approval waits and before returning content.
Close requires the stored Session ID but can release a view after its Location or
read permission changes; it returns no file metadata. No raw bytes enter URLs or
request logs. Project-relative aliases resolve only within the project. Canonical file/directory
bindings and the original alias are revalidated; special files are refused.
Existing text browsing routes remain compatible.

```ts
const document = await client.binary.open(session.id, "fixtures/header.bin");
try {
  const bytes = await document.bytes(0n, 256);
  const matches = await document.search("7f454c46", 0n);
} finally {
  await document.close();
}
```

SDK offsets accept `bigint` or canonical decimal strings, never numbers. The
helper bounds decoding and checks page revision, exact offset, length and digest.
Generated `client.fs.binary*` operations expose the underlying envelopes.

Pages default to 64 KiB and cap at 1 MiB; the TUI requests 4 KiB pages. Search
patterns cap at 4 KiB, with at most 8 MiB of IO, a cooperative 250 ms scan budget
and 100 matches per request. Continuations preserve cross-chunk/slice matches.
Cancellation stops scanning at the next bounded IO chunk; native filesystem IO
itself is not preempted. Up to four documents per Session and sixteen per server
are admitted. Sixteen worker/admission permits bound pending binary IO; saturation
returns an explicit resource-limit refusal. Session deletion releases its live
views; clean abandoned views can be pruned after 24 hours.

The 512 MiB flag in metadata reports only the planned edit-size ceiling. It does
not mean editing is available or that a native save guarantee has been proved.
