# applyPatch specification

`applyPatch(files, patchText, options = {})`, exported from `src/applyPatch.js`.

- `files` is a plain object mapping a path (string) to the file's content (string).
- `patchText` is a unified diff (string).
- `options.reverse` (boolean, default `false`) applies the patch in reverse.
- `options.maxOffset` (non-negative integer, default `100`) bounds the offset search.

It returns either `{ ok: true, report }` or `{ ok: false, error }` (exactly these keys).

**Atomicity.** On success `files` is updated in place: modified and created files get their new
content, deleted files are removed from the object, all other entries are untouched. On failure
`files` is left exactly as it was (no entry added, removed or changed), whichever file section
or hunk failed. `applyPatch` never throws for a malformed patch or a failing hunk.

## Lines

Both file contents and the patch are split into lines at `"\n"` only. A `"\r"` is never removed
or treated specially: it is ordinary line content (so CRLF files are patched by patches whose
lines end in `"\r"`).

- A file's content is a sequence of lines, each including its `"\n"`; the last line may lack the
  `"\n"`. `""` has no lines; `"a\nb"` has the lines `"a\n"` and `"b"`.
- The patch text is split at `"\n"`; if it ends with `"\n"`, the empty string after the final
  `"\n"` is not a line. Patch lines are numbered from 1; every `line N` in an error message is
  such a line number.

## Patch format

A patch is a sequence of **file sections**. Parsing reads the patch top to bottom.

### Outside hunks

Outside a hunk body each line is classified by its start:

- `--- ` starts a file section (see below).
- `@@` is a hunk header for the current file section.
- `\` is an error `line N: misplaced no-newline marker` (a marker is only valid directly after
  a hunk body line, where it is consumed as part of the hunk; see below).
- Anything else (`diff --git ...`, `index ...`, commentary, empty lines) is ignored.

Before the first file section every line other than a `--- ` line is ignored, including lines
starting with `@@` or `\`.

### File headers

A file section starts with a `--- OLD` line that must be immediately followed by a `+++ NEW`
line; otherwise the error is `line N: missing +++ header` where N is the `---` line.

- The path is the text after `--- ` / `+++ ` up to (not including) the first tab or `"\r"`,
  or to the end of the line. A path of `/dev/null` means "no file". Otherwise one leading
  `a/` or `b/` is removed (on either header).
- `--- /dev/null` + `+++ b/P`: **create** `P`. `--- a/P` + `+++ /dev/null`: **delete** `P`.
  Two equal paths: **modify** `P`.
- Both `/dev/null`: error `line N: both paths are /dev/null` (N = the `---` line).
- Two different paths (after removing the prefixes): error `line N: renames are not supported`
  (N = the `---` line).
- Every file section must have at least one hunk. A section without hunks is detected when the
  next `--- ` line or the end of the patch is reached, before that next header is examined;
  the error is `line N: file has no hunks` (N = that section's `---` line).
- A patch with no file section at all gives the error `no file headers found` (no line number).

The same path may appear in several file sections; each section is applied to the result of the
previous ones (a file created by an earlier section exists for later ones; one deleted by an
earlier section does not).

### Hunks

A hunk header has the form `@@ -OS,OC +NS,NC @@`, optionally followed by any text after the
closing `@@` (a section heading). Each of `,OC` and `,NC` may be omitted, meaning a count of 1.
Precisely, the line must match `^@@ -(\d+)(,(\d+))? \+(\d+)(,(\d+))? @@`; in addition a start of
0 is only allowed together with a count of 0. Otherwise: `line N: malformed hunk header`.

- OS/OC describe the old side: with OC > 0, OS is the 1-based number of the first old line;
  with OC = 0 (pure insertion), the new lines go **after** old line OS (OS = 0: at the start).
- NS/NC describe the new side the same way. NS is only checked syntactically when applying
  forwards (it becomes the old side when reversing).

The hunk body follows the header and ends as soon as OC old-side lines and NC new-side lines
have been read (a hunk with OC = NC = 0 has an empty body). While the body is being read every
line belongs to it, even one that starts with `--- `, `+++ ` or `@@`:

- `" "` + text: a context line (counts on both sides).
- `-` + text: a removed line (old side only). `+` + text: an added line (new side only).
- An empty line (`""`): a context line whose text is empty.
- A line that would exceed a remaining count (a context line while either side has no lines
  left, a `-` line with no old lines left, a `+` line with no new lines left): error
  `line N: hunk line exceeds header counts`.
- A line starting with `\` that does not directly follow a body line (e.g. right after the
  header, or a second marker in a row): `line N: misplaced no-newline marker`.
- Any other line: `line N: unexpected line in hunk`.
- If the patch ends before the body is complete: `line N: hunk truncated`, where N is the line
  of the hunk **header**.

**No-newline marker.** A line starting with `\` (conventionally `\ No newline at end of file`)
directly after a body line (including the body's last line) belongs to the hunk and means that
body line has no `"\n"`. It is valid only if that body line is the last old-side line of the
hunk (for context and `-` lines) and the last new-side line of the hunk (for context and `+`
lines); otherwise the error is `line N: misplaced no-newline marker` (N = the marker's line).

A body line's **full text** is its text after the one-character prefix (empty for an empty
line) plus `"\n"`, or without `"\n"` when it carries a no-newline marker. The hunk's **old
lines** are the full texts of its context and `-` lines in order; its **new lines** are those of
its context and `+` lines.

The first parse error in reading order is reported. The whole patch is parsed (and must be
valid) before anything is applied.

## Reverse

With `reverse: true` the patch is parsed exactly as written, then every file section is
inverted before applying: create becomes delete and vice versa; in every hunk the old and new
sides are swapped (OS/OC with NS/NC, `-` lines become `+` lines and vice versa; context lines
and no-newline markers stay with their lines). Sections and hunks are still applied in patch
order.

## Applying a file section

- modify/delete: the file must exist in `files` (as updated by earlier sections), otherwise
  error `P does not exist`. create: it must not exist, otherwise error `P already exists`.
  A created file starts as the empty content `""`.
- Hunks are located and applied in order against the section's **original** lines (the content
  before this section). Positions are 0-based line indexes into those lines; let `n` be their
  number.
- A hunk's **stated position** is `OS - 1` when OC > 0 and `OS` when OC = 0.
- A hunk **matches** at position `p` when `p >= min`, `p + (number of old lines) <= n`, and the
  original lines `p, p+1, ...` are exactly equal to the hunk's old lines (full texts, including
  `"\n"` and any `"\r"`). There is no fuzz: every old line must match exactly. `min` is 0 for the
  first hunk and, for later hunks, the index just past the old lines matched by the previous hunk
  (`p_prev + old line count of the previous hunk`), so hunks can neither overlap nor go backwards.
  A hunk with no old lines matches at every `p` with `min <= p <= n`.
- **Offset search.** Let `center` = stated position + the offset of the previous hunk of this
  section (0 for the first hunk). Try, in this order, `center`, `center - 1`, `center + 1`,
  `center - 2`, `center + 2`, ..., up to `center - maxOffset`, `center + maxOffset`; the first
  position where the hunk matches is used. If none matches, the error is
  `hunk K of P failed`, where K is the 1-based index of the hunk within its file section.
- The hunk's **offset** is the position used minus its stated position (it may be negative).
- The result is the original lines with each hunk's matched old lines replaced by its new
  lines, concatenated.
- delete: after applying, the result must be `""`, otherwise error
  `P is not empty after deletion`; the file is then removed.

In every error message `P` is the section's path (after removing `a/`/`b/`). The first error
encountered (sections in patch order, hunks in order) is returned and nothing is changed.

## Result

On success: `{ ok: true, report }` where `report` has one entry per file section, in patch
order: `{ path: P, action: "modify" | "create" | "delete", offsets: [...] }`, with `offsets`
listing each hunk's offset in order. With `reverse: true` the action is the inverted one.

On failure: `{ ok: false, error: MESSAGE }` with one of the messages above, exactly.
