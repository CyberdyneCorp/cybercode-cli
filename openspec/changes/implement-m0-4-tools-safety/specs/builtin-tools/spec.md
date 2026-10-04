## MODIFIED Requirements

### Requirement: websearch tool
(P0) `websearch` SHALL accept `{ query, max_results? (default 8, max 20), allowed_domains?, blocked_domains? }` and check the `websearch` permission on the query. It SHALL use the model provider's native search when the Turn model's catalog entry declares `capabilities.native_web_search`, and otherwise the configured backend `tools.websearch.backend` (`exa`, `brave`, `searxng`, `parallel`, `firecrawl`, `tavily`, `tinyfish`, or `random`, which rotates among backends with credentials and skips a backend for 10 minutes after it returns 429). When `backend` is unset it SHALL behave as `random`. A backend's credential SHALL resolve from `tools.websearch.<backend>.api_key`, then `CYBER_AUTH_CONTENT` (`{"<backend>": {"key": ...}}`), then the `<BACKEND>_API_KEY` environment variable. `searxng` SHALL be usable when `tools.websearch.searxng.url` or `SEARXNG_URL` is set. `tools.websearch.enabled: false` SHALL hide the tool. Requests SHALL time out after 25 s and responses SHALL be limited to 256 KiB. Results SHALL be returned as `title`, `url`, `snippet`, filtered by the domain lists on whole host labels. Without any backend, the tool SHALL be hidden.

#### Scenario: Native search preferred
- **WHEN** the Turn model declares `native_web_search`
- **THEN** the search is executed by the provider and no third-party backend is contacted

#### Scenario: Rotation after rate limit
- **WHEN** `backend` is `random`, `exa` returns 429 and `brave` has credentials
- **THEN** the query is retried on `brave` and `exa` is skipped for the next 10 minutes

#### Scenario: Hidden without credentials
- **WHEN** no backend has credentials and no SearXNG URL is configured
- **THEN** `websearch` is not offered to the model
