## Context

A tool result reaches the model along one of these paths:

- A built-in call: `BuiltinHost::settle_output` → `Budget::apply_for`.
- A registered call (MCP, browser, plugin, client tool): `bound_registered_output` → `Budget::apply_registered`.
- A user `!` command: `shell_owned` → `Budget::apply`.

The runtime then builds `session.tool.settled` in `drain.rs` and passes it to `Inner::with_call_metadata` before it commits, both for a normal group and for an auto-override replay. The model's history and every client read that event.

Built-in failures (`ToolOutcome::Failed`) skip the budget. Job completion notices (`jobs.rs` `notify_job`) and workflow run summaries (`workflow_results.rs`) are text that the runtime admits into the Session as queued input.

## Goals / Non-Goals

- **Goal:** one redaction policy for every text the model gets from outside the conversation. It is applied before truncation, and its result is the same text that is stored.
- **Goal:** do not fork the credential pattern list. Memory, git text, telemetry and exports use the same detectors.
- **Goal:** redacting a 1 MiB result takes well under 10 ms.
- **Non-goal:** redacting tool input, user-provided context or structured values (see the proposal).

## Decisions

### 1. One module, `cyber_core::redact`

The detectors move from `memory/secrets.rs` into `redact/detectors.rs`, and each detector gets a kind. `memory::redact_credentials(_with)` and the memory `check` delegate to the new module. Those legacy callers use a permissive filter: an assigned value of any length counts, as the old patterns did, and their own marker (`[REDACTED]`, `***`) replaces the kinded one. Tool output uses the stricter filter of decision 3.

The high-entropy check stays in memory only, as before.

### 2. Where redaction runs

There are two passes. Both are idempotent, because a detection that overlaps an existing `[REDACTED:…]` marker is skipped.

1. **The projection pass, before truncation.** `Budget` carries a `Redaction`, which holds the Location's redactor and a counter. `Budget::truncate`, the single truncation path, redacts the raw text first, measures and cuts the *redacted* text, and writes the *raw* text to the overflow file.
   - This puts the step inside the registry projection, before any future untrusted-content framing, which wraps what `truncate` returns. The registry change stays local to `truncate` and two call sites that read the counter.
   - `apply_with_reminder` gets the already-settled output and does not redact again.
2. **The settlement pass.** `with_call_metadata` gets the Location directory and redacts `data.output` through `ToolHost::redact_model_text`. It adds `ToolHost::take_call_redactions`, which is what the projection hid from the full raw text, including any part that truncation removed, and writes `data.redactions`.
   - This also covers built-in failures and instruction or skill reminders.
   - Since this pass runs on the event that is committed and then projected into the model's history, the stored history and the model cannot differ.

There are two more places:

- **`read` redacts the whole file before paging it.** A page boundary inside a PEM block would otherwise show its body lines without the `BEGIN` line, which nothing can recognize. The counter gets only the markers new to the page (`redact::added_markers`).
- **Handbacks.** `notify_job` and the workflow summary embed raw output in JSON. They redact the strings before encoding, then the whole text. Session import redacts imported call outputs, because those outputs reach the model again.

`ToolHost` gets default implementations that apply the built-in detectors. A host without configuration (tests, embedders) still redacts, which keeps the default secure. `AppHost` forwards both methods; an existing test enforces that it forwards every method.

### 3. Detection

The detectors run in priority order. When two matches cover the same text, the earlier kind names it: a GitHub token in `GITHUB_TOKEN=ghp_…` is `github_token`, not `assignment`.

1. Token formats. Each is a regex with a literal prefix, so the scan is a fast literal search. The leading word boundary is ASCII (`(?-u:\b)`), so the scan stays on the lazy DFA, and it also accepts a preceding `\n`, `\r` or `\t` escape, because output is often JSON. A PEM block matches through its `END` line. A block without an `END` line matches through its base64 and header lines only, never through prose.
2. Known exact values, matched by one Aho-Corasick automaton (leftmost-longest, values of at least 8 characters).
   - Environment variables count when their name contains `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `APIKEY`, `ACCESSKEY`, `PRIVATEKEY`, `CREDENTIAL`, `AUTHKEY`, `SESSIONKEY`, `AUTHORIZATION` or `COOKIE`, ignoring `_`, `-` and case. A name that ends in `PATH`, `FILE`, `DIR`, `URL`, `SOCK`, `NAME`, `TTL` and similar does not count, and neither does a value that is a number, a boolean or a path.
   - Config: every string under a secret-named key (provider `api_key`, MCP `env`, `headers.Authorization`). A `Bearer <token>` value adds the bare token too.
   - The catalog's resolved provider keys (`ModelResolver::credential_values`).
   - The values are only matched. `Redactor`'s `Debug` prints their count, never the values.
3. Configured `patterns`; each whole match is `custom`.
4. Generic shapes:
   - `Authorization: Bearer <v>` (`auth_header`);
   - `scheme://user:<v>@` (`url_password`);
   - assignments (`assignment`).

   An assignment value must:
   - be at least 8 characters long;
   - not start with `$`, `{`, `<`, `%`, `[`, `*`, `(` or `#`;
   - not be all digits or one repeated character;
   - contain two of lowercase letters, uppercase letters and digits;
   - not be a dotted or `::` identifier path such as `process.env.X`;
   - contain a digit when it is unquoted after `:` or a spaced `=`, where code such as `token: TokenKind` is common.

   An unquoted value must be followed by a delimiter (whitespace, `,`, `;`, `&`, a closing quote or bracket, a backslash or the end of the line), so `token = self.next()` never matches while `"API_KEY=…"` in a JSON array does. These rules are the false-positive guards that the tests pin: git SHAs, UUIDs, base64 images, lockfile `integrity`/`checksum`, `max_tokens: 4096`, placeholders, schema strings and source code.

### 4. Marker

The marker is `[REDACTED:<kind>]`, with a kind in `[a-z0-9_]`. It is one stable format for the model, the write refusal and clients. A match that spans line breaks is followed by the same number of `\n`, so `read` line numbers and paging are those of the raw file.

### 5. Writes

- `fs::guarded_write`, which `write`, `edit` and `notebook_edit` share, refuses before it asks for permission when the new text holds a marker more often than the current text does (`redact::introduced_marker`). A documentation file that already shows a marker can therefore still be edited.
- `apply_patch` compares all the patch's after-texts with all its before-texts, so moving a file is not an addition.
- Shell input is refused whenever it holds a marker. A heredoc, `sed -i` or redirect could only write the placeholder, and anything else would send it where the secret was expected. The error says that a value is hidden and how to refer to it indirectly.
- An `edit` whose `old_string` holds a marker gets a hint that the file has the real value.

### 6. Settings and project layers

`RedactSettings::from_config` validates `tool_output.redact`:

- `enabled` is a boolean;
- `patterns` has at most 32 entries and `allow` at most 64;
- each entry is a non-empty string of at most 1024 bytes;
- regexes compile with a 256 KiB size limit and a 1 MiB DFA limit, and may not match empty text;
- an `allow` entry is a regex when it is written `/re/`, and a literal otherwise;
- unknown keys are errors.

The config loader validates the merged document, so a bad pattern fails loading with `tool_output.redact.patterns[<i>]: …`.

The loader's per-layer step (`restrict_layer`, which also strips `mcp_serve`) drops `enabled` (unless `true`) and `allow` from `project:` and `plugin:` layers, at the top level and in their `profiles`, and warns. It does this whatever the trust state: trust approves executing a checkout's integrations, not hiding fewer secrets from the provider.

`/tool_output/redact/patterns` and `/allow` join `CONCAT_KEYS`, so a project's `patterns: []` cannot erase the user's patterns. A project pattern needs no trust, because it can only hide more. Its cost is bounded by the regex limits, and the engine is linear-time.

The host builds a `Redactor` per resolved config and caches the last one by (settings, known values). An unreadable config falls back to the defaults, which fails closed.

### 7. Performance

Each detector scans once without allocating. Spans are collected, sorted by (start, longest, priority), de-overlapped, and the text is rebuilt once. Text without a detection is returned borrowed.

The assignment detector, which has no literal prefix and is dense in JSON with `token` keys, is split by hand instead of through the capture engine; captures made a dense megabyte 2.5 times slower. The regex, aho-corasick and memchr crates are built with `opt-level = 3` in dev and test profiles, like the image crates.

Measured on a dev machine for 1 MiB, with cyber-core unoptimized: 3 ms for log lines, 8 ms for JSON dense with `token` keys. The test asserts the best of 5 runs is under 10 ms in release and under 50 ms in debug builds.

### 8. Visibility

`ToolSettled.redactions` and `CallState.redactions` are `Option<Redactions { count, kinds }>`, omitted when nothing was hidden. Clients show them as follows:

- the TUI shows `⊘ 2 secrets hidden from the model` under the tool card;
- the dashboard shows an amber badge in the tool summary;
- `exec` adds `redactions` to the `tool_result` stream line and prints `<tool>: 2 secrets hidden from the model` to stderr in text mode.

## Risks / Trade-offs

- **False positives in source code.** The assignment guards trade recall for precision. A lowercase-only password such as `secret = "changeme"` is not redacted. The provider formats, exact values and PEM blocks do not depend on these guards.
- **Shell refusal is broad.** A command that only greps for a marker is refused too. A marker never exists in real files unless a write put it there, and writes refuse that.
- **Merging with `add-untrusted-content-defense`.** That change also edits `Budget::truncate`, the base prompt (version 3) and `ToolSettled`. The merge keeps the redaction first in `truncate`, bumps the prompt version to 4, and keeps both new event fields.

## Migration

None. The prompt version bump starts a new Context Epoch for Sessions that continue.

## Overlapping detection correction

Merge review reproduced known-secret suffix exposure when a built-in token and
an exact known value intersect, and when two known values intersect. Nonoverlapping
literal matching followed by dropping intersecting spans left the unmatched tail
visible. Detection now enumerates overlapping known values and coalesces the full
coverage before reconstruction, retaining the most specific detector kind.
Known coverage coalesces during enumeration instead of storing every overlapping
match. Existing markers and allow entries are filtered before coalescing, so
allowed individual values remain exempt. Regressions include built-in/known,
known/known, self-overlapping and Unicode values, plus overlapping allow entries.

## Composition with untrusted-content defense

The combined prompt version is 4 and retains both guidance topics. Redaction
runs before output truncation and untrusted framing; raw managed overflow files
retain their provenance. Tool settlements and client cards retain both redaction
counts and typed injection records. The runtime also redacts the injection
record's origin and excerpts before committing it or retaining it as taint for
later classifier evidence, without changing detector kinds/version or exceeding
its existing bounds. A real-runtime regression reproduced a known secret in that
metadata while the ordinary result body was already redacted, then verifies the
corrected stored call and derived taint. This is independent of the overlap fix.

The webfetch summarizer is also a provider boundary. Its page and provenance
origin now pass through the Location redactor before clipping and framing; the
user/model-supplied summary request keeps its existing input semantics. The
real HTTP/scripted-summary regression failed before this correction because the
auxiliary request contained both a token-shaped credential and an exact known
value. A page-cut regression additionally protects a secret crossing the
100,000-character summary boundary.

Fetched text is redacted before HTML-to-text/Markdown conversion as well, since
conversion can escape token punctuation. Redaction counts are retained and
Markdown-escaped placeholder spellings are restored to their canonical marker.
The render regression covers raw HTML, plain text and Markdown output.
