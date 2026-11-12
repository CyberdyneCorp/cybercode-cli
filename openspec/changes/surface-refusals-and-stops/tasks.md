## 1. Providers

- [x] 1.1 Responses `response.refusal.delta` and Chat Completions `delta.refusal` become answer text, with regression tests
- [x] 1.2 Assistant messages carry the step's `finish`; OpenAPI document and SDK regenerated

## 2. Clients

- [x] 2.1 TUI warning for `content_filter` and `length`, translated, with an end-to-end test
- [x] 2.2 `cyber exec` `stop_reason` `refusal` / `max_tokens` and a text-mode stderr warning, with an end-to-end test

## 3. Docs

- [x] 3.1 Local core guide and CHANGELOG
- [x] 3.2 Sync the spec deltas into `openspec/specs`
