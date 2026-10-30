# Untrusted-content injection fixtures

Labeled samples for `untrusted-content` → Injection defense evaluation. They are scanned with
the built-in rule table by `cargo test -p cyber-core --test untrusted_eval` (add
`-- --nocapture` for the report).

- `injection.jsonl`: one object per line, `{id, delivery, kind, text}`. `kind` is the signal the
  sample is written to carry (it may raise others); `delivery` is `webfetch`, `websearch`,
  `mcp` or `repository`. At least 5 samples per kind, at least 60 in all.
- `benign.jsonl`: `{id, delivery, category, text}` with `category` `docs`, `readme`, `search`,
  `mcp` or `security`. At least 200 samples, 20 of them `security` articles about prompt
  injection, which are reported separately and not counted as false positives.

`text` is what the model sees inside the frame: a converted page, a rendered `websearch` list
(`N. title`, url, snippet) or an MCP result. A `repository` sample is a file's contents; the test
numbers its lines as `read` does, since `scan: "all"` scans `read` output.

The gate: overall recall of at least 90% on the injection samples, and at most a 2%
false-positive rate on the benign samples other than the security articles. A failing gate
lists the missed samples by kind. Hidden characters are written as JSON `\u` escapes so they
stay visible in review.

Do not tune samples to the rules: a miss or a false positive is a finding about the rules.
Samples are original text written for this set.

Every sample verifies `untrusted-content` → Injection signal detection; the gate itself verifies
Injection defense evaluation. The report names both, and the two gate tests are listed as cases
of `eval/manifests/recovery-storage.json`.

The benign set includes install guides that pipe a download to a shell, build files that
`rm -rf` a build tree, API documentation that sends a key in a header, and REST route templates
(`/users/{username}`): the rule table must not flag them (detector v2).
