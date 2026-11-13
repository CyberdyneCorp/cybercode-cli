## Why

When a provider request fails, users cannot tell a bad key from a network that blocks the host, a proxy that refuses the connection, or a local server that is not running. `cyber doctor` only checks that credentials exist. Codex added `codex doctor --network-only` for the same reason (openai/codex#57606). This is a clean-room equivalent built on our own catalog and HTTP client.

## What Changes

- `cyber doctor --network` runs only network checks:
  - a `proxy` line listing the proxy variables in effect, with proxy credentials hidden;
  - one `network <provider>` line per enabled provider that has a key or a subscription login, and per configured endpoint that needs no credential;
  - a `network updates` line for the update channel.
- Probes use the provider HTTP client settings (proxy, user agent, no redirects), send no credentials, time out after 10 seconds and run concurrently.
- Any HTTP response passes. A connection, DNS, TLS or timeout error fails, but only warns for local endpoints and the update channel.
- Plain `cyber doctor` is unchanged.

## Impact

- Spec: `cli-commands` (added requirement).
- Code: `cyber-cli` doctor command and its `--network` flag.
- Docs: `docs/cli.md`, local core guide, CHANGELOG.
