## ADDED Requirements

### Requirement: Tool output redaction
(P1) The system SHALL redact secrets from the text of every tool result before it reaches the model. This covers built-in tools (`read`, `grep`, `bash`, `powershell`, Job output reads, `webfetch`, `websearch`), MCP, browser, plugin and client tools, subagent results, user `!` shell output, Job completion notices and workflow run summaries.

The registry projection SHALL redact the raw result text first. Truncation (Output size budget) SHALL then measure and cut the redacted text, so a secret is hidden whole wherever the cut falls. Any later framing of the result SHALL wrap the redacted, truncated text.

The runtime SHALL redact the settled output once more, including failures and appended reminders, and SHALL store exactly that text as the `session.tool.settled` `output`. The model's history, the TUI, the web client, exports and replay therefore SHALL never differ. Redaction SHALL be idempotent: an existing marker is never matched again.

A managed overflow file and a Job log SHALL keep the raw, unredacted text on the local disk. Reading either back through a tool SHALL redact it again. `read` SHALL redact the whole file before it selects a page, so a secret cut by a page boundary stays hidden.

A Job completion notice and a workflow run summary SHALL redact the strings they embed before encoding them as JSON. An imported Session's call outputs SHALL be redacted on import.

Tool input that the model wrote, structured values and user-provided context (typed prompts, pasted text, `@` mentions, the text of `!` commands) SHALL NOT be redacted. `tool_output.redact.enabled: false` in user config SHALL turn redaction of tool output off.

#### Scenario: Shell output
- **WHEN** the model runs `bash` with `export GITHUB_TOKEN=$(cat token.txt); echo "token is $GITHUB_TOKEN"` and the file holds `ghp_…`
- **THEN** the model's tool result and the stored settlement both read `token is [REDACTED:github_token]`

#### Scenario: Credentials file
- **WHEN** the model reads an `.npmrc` with `//registry.npmjs.org/:_authToken=npm_…`
- **THEN** the result shows `:_authToken=[REDACTED:npm_token]` and keeps the other lines

#### Scenario: MCP result
- **WHEN** an MCP tool returns text with a Slack token and the value of a secret from its own `env`
- **THEN** the model and the stored settlement get `[REDACTED:slack_token]` and `[REDACTED:known_secret]`

#### Scenario: Overflowed result
- **WHEN** a `bash` result exceeds the output budget and its kept tail holds a PEM private key
- **THEN** the preview shows `[REDACTED:private_key]`, the overflow file holds the raw key, and a `read` of the overflow file shows the marker again, even for a page that starts inside the key

#### Scenario: Job handback
- **WHEN** a background `bash` Job prints a GitLab token and ends
- **THEN** the completion notice admitted for the model has `[REDACTED:gitlab_token]` in `output_tail`, and the Job log keeps the token

### Requirement: Secret detection
(P1) The system SHALL detect secrets with one shared detector list in priority order; the earliest kind names a match that several detectors find. The list SHALL be:

1. Credential formats:
   - AWS access keys (`AKIA…`, `ASIA…`);
   - GitHub tokens (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`);
   - Anthropic (`sk-ant-`) and OpenAI (`sk-`, `sk-proj-`, `sk-svcacct-`, `sk-admin-`) keys;
   - Slack tokens (`xox[abposr]-`);
   - Stripe live keys (`sk_live_`, `rk_live_`);
   - Google API keys (`AIza`);
   - GitLab (`glpat-`) and npm (`npm_`) tokens;
   - GitHub Copilot API tokens, peer grant tokens (`cyp_`) and JWTs;
   - PEM `-----BEGIN … PRIVATE KEY-----` blocks, through their `END` line, or through their base64 and header lines when the `END` line is missing.
2. Exact values of at least 8 characters of known secrets:
   - environment variables of the `cyber` process whose names look secret (containing `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `API_KEY`, `ACCESS_KEY`, `PRIVATE_KEY`, `CREDENTIAL`, `AUTHORIZATION`, `COOKIE` or similar, not ending in `_PATH`, `_FILE`, `_DIR`, `_URL` or similar);
   - string values under secret-named config keys, such as `providers.<p>.api_key`, MCP server `env` and `headers`, with the bare token of a `Bearer <token>` value;
   - the resolved provider API keys.
3. The configured `tool_output.redact.patterns`.
4. `Authorization` header values, URL passwords (`scheme://user:<password>@`), and assignments `NAME=value` or `"name": "value"` whose name contains `token`, `secret`, `password`, `api_key`, `access_key`, `credential`, `private_key`, `auth_key`, `session_key` or `client_key`.

An assigned value SHALL be redacted only when it:
- is at least 8 characters long;
- does not start with `$`, `{`, `<`, `%`, `[`, `*`, `(` or `#`;
- is not all digits or one repeated character;
- contains two of lowercase letters, uppercase letters and digits;
- is not a dotted or `::` identifier path;
- contains a digit when it is unquoted after `:` or a spaced `=`;
- when unquoted, is followed by whitespace, `,`, `;`, `&`, a closing quote or bracket, a backslash or the end of the line.

A token detector SHALL also match right after a `\n`, `\r` or `\t` escape.

The system SHALL NOT redact text only because it has high entropy. A value matching an `allow` entry SHALL be kept. Known values SHALL never be logged or printed.

Redacting 1 MiB SHALL take well under 10 ms on a developer machine.

#### Scenario: Hashes are not secrets
- **WHEN** output contains a git SHA, a UUID, a base64 PNG data URL and lockfile `integrity` and `checksum` values
- **THEN** nothing is redacted and the text is returned unchanged

#### Scenario: Code is not an assignment
- **WHEN** a source file contains `token = self.next_token()`, `fn lex(token: TokenKind)` and `"max_tokens": 4096`
- **THEN** none of them is redacted, while `DB_PASSWORD=Hunter2Hunter2` in the same file becomes `DB_PASSWORD=[REDACTED:assignment]`

#### Scenario: Known secret values
- **WHEN** `DEPLOY_TOKEN=plain-deploy-value-0042` is set in the server's environment, `providers.corp.api_key` is configured, and a file printing both values is read
- **THEN** each value is replaced by `[REDACTED:known_secret]`

### Requirement: Redaction markers
(P1) The system SHALL replace each redacted value with `[REDACTED:<kind>]`, where `<kind>` is one of:
- `aws_access_key`, `github_token`, `anthropic_key`, `openai_key`;
- `slack_token`, `stripe_key`, `google_api_key`, `gitlab_token`;
- `npm_token`, `copilot_token`, `peer_grant`, `jwt`, `private_key`;
- `known_secret`, `custom`;
- `auth_header`, `url_password`, `assignment`.

An assignment SHALL keep its name, separator and quotes, so that only the value is replaced. A redacted value that spans line breaks SHALL be followed by the same number of line breaks, so line numbers and paging do not change.

#### Scenario: Private key keeps its lines
- **WHEN** a 4-line PEM block sits on lines 2 to 5 of a file
- **THEN** line 2 reads `[REDACTED:private_key]`, lines 3 to 5 are empty, and line 6 is the file's line 6

### Requirement: Redaction visibility
(P1) A settlement that hid anything SHALL carry `redactions: { count, kinds }`, with the number of hidden values and their distinct kinds, sorted. This applies to the `session.tool.settled` event and to the Session's call state, and so to `CallState` in the HTTP API and the SDK.

The count SHALL include values in parts of the raw result that truncation removed. A settlement that hid nothing SHALL omit the field.

#### Scenario: Count on the event
- **WHEN** a `bash` result has one GitHub token and one database password
- **THEN** the settlement has `"redactions": {"count": 2, "kinds": ["assignment", "github_token"]}`
