# Template language specification

`render(template, view, partials = {})` (exported from `src/render.js`) renders `template` (a
string) against `view` (any value) and returns the output string. `partials` maps partial
names to template strings. This document is the authoritative contract; anything it does not
mention is unspecified and not tested. `render` never modifies `view` or `partials`, and it must
render large inputs (for example a section over a 20,000-element list) well within a second.

## 1. Tags

A tag starts with the current *opening delimiter* and ends at the **first** occurrence of the
current *closing delimiter* after it. The delimiters are `{{` and `}}` at the start of every
template (see section 8 for changing them). Everything outside tags is literal text and is
copied to the output unchanged (including a lone `}}`).

The *content* of a tag is the text between the delimiters with leading and trailing whitespace
removed. The tag type is decided by the first character of the content:

| First character | Type | Name |
|---|---|---|
| `#` | section open | rest of the content, trimmed |
| `^` | inverted section open | rest of the content, trimmed |
| `/` | section close | rest of the content, trimmed |
| `!` | comment | (none; the rest is ignored) |
| `>` | partial | rest of the content, trimmed |
| `&` | unescaped variable | rest of the content, trimmed |
| `=` | set delimiters | see section 8 |
| anything else | escaped variable | the whole content |

So `{{ name }}`, `{{# list }}`, `{{ #list }}` and `{{/ list }}` are all valid.

**Triple mustache.** Only while the current delimiters are exactly `{{` and `}}`: an opening
`{{` immediately followed by `{` starts an unescaped-variable tag that ends at the first `}}}`
after it; its name is the text between `{{{` and `}}}`, trimmed. `{{{ name }}}` is equivalent
to `{{& name }}`.

## 2. Names and lookup

The renderer keeps a *context stack*. It starts as `[view]`; sections push values onto it
(section 5) and pop them afterwards.

A *frame has a key* when the frame is a non-null object (arrays included) and the key is an
**own** property of it (`Object.hasOwn`). Primitives, `null`, `undefined` and functions have no
keys, and inherited properties such as `toString` are never found.

Resolving a name:

1. The name `.` resolves to the top of the stack (the implicit iterator).
2. Any other name is split on `.` into segments. The first segment is looked up by walking the
   stack from the top (innermost) frame outwards; the first frame that has the key wins, **even
   if the value stored there is `null`, `undefined` or `false`**. If no frame has it, the name
   is missing.
3. Each further segment is looked up **only** in the value obtained so far (it must have the key
   in the sense above); if it does not, the whole name is missing. There is no fallback to
   outer frames for these segments. Example: with `{a: {b: {}}, b: {c: "X"}}`,
   `{{#a}}{{b.c}}{{/a}}` renders nothing.
4. A function value is never called: if resolution yields a function (at any step, including
   `.`), the name is missing. Lambdas are not supported.

A missing name resolves to `undefined`. Names containing empty segments (`a..b`, `.a`, `a.`)
are unspecified.

## 3. Variables

`{{name}}` resolves the name, converts the value to a string and HTML-escapes it.
`{{& name}}` and `{{{name}}}` do the same without escaping.

String conversion: `undefined` and `null` become the empty string; strings are used as they
are; every other value becomes `String(value)` (so `0` -> `0`, `1.5` -> `1.5`, `true` ->
`true`, `false` -> `false`, `[1, 2]` -> `1,2`).

Escaping replaces exactly these five characters and nothing else:

| Character | Replacement |
|---|---|
| `&` | `&amp;` |
| `<` | `&lt;` |
| `>` | `&gt;` |
| `"` | `&quot;` |
| `'` | `&#39;` |

Variable tags are never standalone (section 7): whitespace and newlines around them are kept.

## 4. Truthiness

A value is **falsy** when it is `undefined`, `null`, `false`, the empty string `""`, an empty
array, or missing (which includes functions, section 2). **Everything else is truthy,
including the number `0`, `NaN`, the string `"0"`, and the empty object `{}`.**

## 5. Sections

`{{#name}}` ... `{{/name}}` encloses a block. The name is resolved once, then:

- falsy value: the block renders nothing;
- non-empty array: the block is rendered once per element, in order, with that element pushed
  onto the context stack (elements may be any value, including arrays and primitives);
- any other truthy value (object, non-empty string, number, `true`, ...): the block is rendered
  once with that value pushed onto the context stack.

Sections nest, and a section may reuse the name of an enclosing one.

## 6. Inverted sections and comments

`{{^name}}` ... `{{/name}}` renders its block exactly once, without pushing anything, when the
value is falsy, and renders nothing otherwise.

`{{! text }}` renders nothing. The text may span several lines and contain anything except the
current closing delimiter.

## 7. Standalone lines

A *line* is a stretch of the template from its start or just after a `\n`, up to and including
the next `\n` (or up to the end of the template). Only `\n` starts a new line; a `\r` that is
followed by `\n` is part of the `\r\n` line ending.

A tag of type section open, inverted open, section close, comment, partial or set delimiters is
**standalone** when

- everything between the start of its line and the tag's opening delimiter is spaces and tabs
  only (possibly nothing), and
- everything after the tag's closing delimiter up to the next line ending is spaces and tabs
  only, and that line ending is `\n`, `\r\n` or the end of the template.

For a multi-line tag (a comment) the first condition is checked on the line where the tag
starts and the second on the line where it ends. Consequently a line with two tags on it is
never standalone.

A standalone tag removes the whitespace before it, the whitespace after it and the line ending
that follows it (`\n` or `\r\n`); nothing of that line reaches the output. Example:
`"a\n  {{#x}}  \r\nb\n{{/x}}\nc"` with `x` true renders `"a\nb\nc"`.

## 8. Set delimiters

`{{=<% %>=}}` changes the delimiters. The tag content (trimmed) must start and end with `=`;
between them, after trimming, there must be exactly two whitespace-separated parts, the new
opening and closing delimiter, and neither may contain `=` (whitespace cannot occur in them by
construction). Otherwise rendering fails with `Invalid delimiters` (section 10). `{{= | | =}}`
is valid and sets `|` and `|`.

The new delimiters apply from right after the tag to the end of the template that contains it,
across section boundaries (they are not scoped to sections). Every template — including every
partial each time it is rendered — starts with `{{` and `}}`; delimiter changes inside a partial
do not affect the template that included it and vice versa.

## 9. Partials

`{{> name}}` renders the partial `partials[name]` with the **current** context stack and inserts
its output unescaped. The name is used as a literal key (dots have no special meaning). If `partials` has no own property `name` or its value is not a string,
the tag renders nothing. Partials may include partials, including themselves (recursion ends
through the data).

When the partial tag is standalone, the whitespace that preceded it on its line is its
*indentation*: before the partial is parsed, the indentation is inserted at the start of the
partial's template and after every `\n` in it except a `\n` that is the template's last
character. An empty partial stays empty. The standalone line itself is removed as in section 7,
so with `partials = {p: "a\nb\n"}`, `"x\n  {{>p}}\ny"` renders `"x\n  a\n  b\ny"`. Data
inserted by variables inside the partial is not indented. A non-standalone partial tag gets no
indentation.

A partial is parsed only when it is rendered (an unused or never-reached partial is never
parsed).

## 10. Errors

Parsing reads the template from left to right and the first error encountered is thrown as a
plain `Error` whose `message` is exactly one of the following (`N` and `M` are 1-based line
numbers of the opening delimiter of the offending tag; line numbers count `\n` characters; for
a template that is a partial they are relative to the partial's own template after
indentation). A template that throws produces no output.

| Situation | Message |
|---|---|
| an opening delimiter with no closing delimiter after it (for `{{{`, no `}}}`) | `Unclosed tag at line N` |
| a variable, section, inverted, close or partial tag whose name is empty | `Empty tag at line N` |
| a set-delimiter tag that breaks section 8 | `Invalid delimiters at line N` |
| a close tag when no section is open | `Unexpected closing tag "NAME" at line N` |
| a close tag whose name differs from the innermost open section's name | `Closing tag "NAME" at line N does not match "OPEN" opened at line M` |
| the template ends while a section is still open | `Unclosed section "OPEN" at line M` |

For the last row, `OPEN`/`M` are the innermost (most recently opened) still-open section. Errors
are detected while parsing, so they are thrown even if the offending part would render nothing
for the given view (for example inside a false section); a partial's errors are only detected
when it is rendered.
