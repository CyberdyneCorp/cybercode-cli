## 1. Build identity

- [x] 1.1 `cyber-core` build script: `CYBER_BUILD_ID` override, commit plus `.dirty`, else build time (honoring `SOURCE_DATE_EPOCH`); `BuildInfo::build` and `build_id_source()`; a unit test for each source.
- [x] 1.2 Executable stamp (`version::binary()`): canonical path, length and modification time, read once at startup by `cyber`.

## 2. Server

- [x] 2.1 `/health` reports `build`; `server.json` records `build`, `binary` and `binary_stamp`.
- [x] 2.2 Authenticated `GET /api/v1/service`: id, version, build, pid, binary, running Sessions, active workflow runs; OpenAPI and the regenerated TS SDK.

## 3. Service start

- [x] 3.1 Reuse only a server of this version and build (and of this stamp for the same executable path).
- [x] 3.2 Replace a stale server when it runs this install's executable and reports no running work; keep it with a warning otherwise, and when it cannot report its work.
- [x] 3.3 `service status` reports `stale`; the CLI prints the notice on stderr for every client path.
- [x] 3.4 Real-binary tests (`cyber-cli` `service_build`): an idle stale server is replaced; a busy one is kept with the warning and `status` says `stale`; another install's server, or one without `GET /service`, is never stopped.

## 4. Clients

- [x] 4.1 `cyber-client`: an undeclared `404` is `RouteNotFoundError`.
- [x] 4.2 TUI: the sidebar warning, `needs a newer server · cyber service restart` in the Machine section and in panels on newer routes, and the Status tab build comparison; a screens test against the production App.
- [x] 4.3 Docs: the service section of `docs/local-core-guide.md`.
