## Why

After rebuilding `cyber`, clients kept reusing the background server started by the old binary: both reported the same version string, and `cyber service start` reused any registered server whose `/health` matched the version. Routes added since that server started were missing. For example, the TUI's Machine section showed `—` because the old server had no `GET /api/v1/system`.

## What Changes

- Every binary embeds a build identity: `CYBER_BUILD_ID` when set, otherwise the commit (with `.dirty` when tracked files differ), otherwise the build time (`SOURCE_DATE_EPOCH` when set). At startup it also stamps its executable (canonical path, length and modification time).
- `server-api` "Health and version": `/health` reports `build`. `server.json` adds `build`, `binary` and `binary_stamp`. New authenticated `GET /api/v1/service` returns the server's identity and build, its running Sessions and its active workflow runs.
- `server-api` "Background service management": `start` reuses a registered server only when its version and build match the client and, for the same executable path, its stamp too. A stale server is stopped and replaced only when it runs this install's executable and `GET /service` reports no running Turns or workflow runs. The client then prints `restarted the Cyber server: it was running an older build`. Otherwise the server is kept and the client prints `The Cyber server runs an older build; restart it with `cyber service restart` when idle`. `status` reports `stale`. Every client path (TUI, `exec`, `api`, `mcp serve`, `sessions`, `stats`, `workflows`, `worktree`) starts the service the same way.
- `client-sdk`: an undeclared `404` from the server is `RouteNotFoundError` rather than a malformed response.
- `tui` (in the open `improve-tui-experience` change): the sidebar warns `⚠ older server build · cyber service restart` when `/health` reports another build. The Machine section and the other panels on newer routes show `needs a newer server · cyber service restart` instead of `—`. The Status tab version row warns when the builds differ.

## Impact

`crates/cyber-core` (build id, executable stamp), `crates/cyber-server` (`/health`, `GET /service`), `crates/cyber-app` (registration and service start), `crates/cyber-cli` (notices, `service status`), `crates/cyber-client` (`RouteNotFoundError`), `crates/cyber-tui` (stale-server hints), and the OpenAPI document and TS SDK.

A server that predates this change reports no build and has no `GET /service`. Clients therefore keep it and warn once; after one `cyber service restart`, later rebuilds are detected and replaced automatically.
