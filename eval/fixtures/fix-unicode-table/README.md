# termtable

Small terminal table and text layout helpers (Python 3.10+, standard library only). They are
used by our CLI tools to print reports whose cells contain user data in any language, so all
measuring is done in **terminal columns**, never in code points.

```python
from termtable import render_table

print(render_table([["Tokyo", "東京", "13960000"], ["Zürich", "チューリッヒ", "421878"]],
                   headers=["city", "local", "population"], align=["left", "left", "right"]))
```

Run the tests with `python3 -m unittest`. A tiny CLI renders a TSV file (first line is the
header): `python3 -m termtable report.tsv --max-width 60 --align l,r --wrap`.

## Contract

This section is authoritative. "Width" always means display width as defined below.

### 1. Normalization (`normalize(text) -> str`)

Every public function that takes text (`display_width`, `graphemes`, `truncate`, `align`,
`wrap` and every cell and header of `render_table`) first normalizes it, and all text it
returns is normalized text:

1. Apply Unicode NFC normalization (`unicodedata.normalize("NFC", text)`).
2. Then expand each tab character (U+0009) to the number of spaces (1 to 4) that brings the
   width of the text before the next character to the next multiple of 4. The width before a
   tab is measured on the NFC text with the rules of section 2, so a wide character counts 2
   and a combining mark counts 0: `"ab\tc"` becomes `"ab  c"`, `"abcd\tc"` becomes
   `"abcd    c"`, `"中\tc"` becomes `"中  c"` and `"e\u0301\tc"` becomes `"é   c"`.

Input text never contains line breaks (`\n`, `\r`) or other control characters except tab.

### 2. Character and display width

`char_width(ch) -> int` for a single character other than tab:

* **0** if the character is U+200B ZERO WIDTH SPACE, U+200D ZERO WIDTH JOINER or U+FEFF ZERO
  WIDTH NO-BREAK SPACE, or its general category (`unicodedata.category`) is `Mn` (nonspacing
  mark), `Me` (enclosing mark) or `Cf` (format character). This rule is checked first, so a
  combining mark is 0 even if its East Asian Width is W.
* otherwise **2** if its East Asian Width (`unicodedata.east_asian_width`) is `W` (wide) or `F`
  (fullwidth);
* otherwise **1** (this includes East Asian Width `A` ambiguous, `H` halfwidth, `N` and `Na`).

`display_width(text) -> int` is the sum of `char_width` over the normalized text.

### 3. Graphemes

For this module a **grapheme** is a character with width 1 or 2 followed by all the
zero-width characters (width 0) that come directly after it. Zero-width characters at the very
start of a text, before any other character, form one grapheme of width 0 on their own.
`graphemes(text) -> list[str]` returns the graphemes of the normalized text in order (their
concatenation is the normalized text). The width of a grapheme is the sum of its characters'
widths. Text is only ever cut **between** graphemes: no function may separate a character from
the zero-width characters that follow it, and no function may cut a wide character in half.

### 4. Truncation (`truncate(text, width) -> str`)

`width` must be an integer >= 1, otherwise `ValueError`. If the normalized text fits
(`display_width <= width`) it is returned unchanged. Otherwise the result is: the longest
prefix of whole graphemes whose width is at most `width - 1`, then a single space if that
prefix is only `width - 2` wide (this happens when the next grapheme is a wide character that
would overflow by one column), then the ellipsis `"…"` (U+2026, width 1). A truncated result
therefore always has width exactly `width`. Spaces are not stripped: `truncate("ab cd", 4)` is
`"ab …"`. Examples: `truncate("hello world", 8) == "hello w…"`,
`truncate("中文字幕", 6) == "中文 …"`, `truncate("中文字幕", 7) == "中文字…"`,
`truncate("anything", 1) == "…"`.

### 5. Alignment (`align(text, width, how="left") -> str`)

`width` must be an integer >= 1 and `how` one of `"left"`, `"right"`, `"center"`, otherwise
`ValueError`. The normalized text is first truncated to `width` (section 4), then padded with
spaces (U+0020) to width exactly `width`: `left` puts all padding on the right, `right` puts it
all on the left, `center` puts `padding // 2` spaces on the left and the rest on the right (so
the extra space of an odd padding goes on the **right**): `align("ab", 5, "center") == " ab  "`.

### 6. Wrapping (`wrap(text, width) -> list[str]`)

`width` must be an integer >= 2, otherwise `ValueError`. The normalized text is split into
**words** at space characters (U+0020) only; a run of several spaces counts as one separator
and leading/trailing spaces are ignored. Nothing else is a break opportunity: hyphens, U+00A0,
U+3000, U+200B and other characters are part of words. Lines are filled greedily:

* A word is appended to the current line, separated by one space, if the resulting line is at
  most `width` wide; otherwise the current line is finished and the word starts a new line.
* A word wider than `width` always starts on a new line (finishing the current one, if it is
  not empty) and is hard-broken into chunks: each chunk is the longest prefix of whole
  graphemes of the remaining word whose width is at most `width`. Every chunk but the last is
  a finished line; the last chunk becomes the current line, and later words may be appended
  to it by the rule above.

Lines are not padded and contain no leading or trailing spaces. Text without words returns
`[]`. Example: `wrap("the quick brown fox", 10) == ["the quick", "brown fox"]`,
`wrap("abcdefgh ij", 3) == ["abc", "def", "gh", "ij"]`, and
`wrap("well-known 中文字", 5) == ["well-", "known", "中文", "字"]`.

### 7. Column widths (`allocate(natural, max_width) -> list[int]`)

`natural` is the list of natural column widths; it is never modified, and the result is
always a new list (a copy of `natural` when `max_width` is `None`). With a `max_width`, the
table width is `sum(widths) + 3 * (len(widths) - 1)` (columns are
separated by `" | "`). While the table width exceeds `max_width`, the **widest** column whose
width is greater than 2 is narrowed by one column; if several columns share that greatest
width, the **rightmost** of them is narrowed. Columns are never narrowed below 2 (a column whose
natural width is 1 or 2 is never narrowed). If the table still does not fit when no column can
be narrowed, raise `ValueError`.

Example: `allocate([10, 4, 10], 20) == [5, 4, 5]`: the table is 30 wide, so ten columns are
taken away one at a time from the widest column, rightmost first:
`[10, 4, 10] -> [10, 4, 9] -> [9, 4, 9] -> [9, 4, 8] -> ... -> [5, 4, 5]`.

### 8. Tables (`render_table(rows, headers=None, *, max_width=None, align=None, overflow="truncate") -> str`)

* `rows` is a sequence of rows, each a sequence of cell strings; `headers` is an optional
  sequence of header strings. All rows and the headers must have the same number of cells
  (at least one), otherwise `ValueError`. With no rows and no headers the result is `""`.
* `align` is `None` (every column `"left"`) or a list with one of `"left"`, `"right"`,
  `"center"` per column, otherwise `ValueError`. Header cells use their column's alignment.
* `overflow` is `"truncate"` or `"wrap"`, otherwise `ValueError`.
* The natural width of a column is the greatest `display_width` of its cells, headers
  included, and at least 1. Column widths are `allocate(natural, max_width)`, so a table that
  cannot fit `max_width` raises `ValueError`.
* Each output line is the column cells joined by `" | "`, every cell exactly as wide as its
  column (the last column too: lines are not right-stripped). With headers, the header line is
  followed by a separator line made of `"-" * width` for each column joined by `"-+-"`.
* `overflow="truncate"`: each cell (and header) is rendered as `align(cell, column_width, how)`
  and every row is one line.
* `overflow="wrap"`: a cell (or header) that fits its column (`display_width(cell) <=
  column_width`) is the single line `cell`; a wider cell is `wrap(cell, column_width)`, or
  `[""]` when that is empty (a narrowed column is always at least 2 wide). A row is as many
  lines as its tallest cell; shorter cells are padded with empty lines at the bottom; every
  line of every cell is rendered with `align(line, column_width, how)`.
* The lines are joined with `"\n"`, without a trailing newline.

Example:

```text
>>> print(render_table([["中文", "1"], ["abc", "22"]], headers=["name", "n"]))
name | n 
-----+---
中文 | 1 
abc  | 22
```
