## Why

When an OpenAI model refuses, its answer arrives as refusal content (`response.refusal.delta` on Responses, `delta.refusal` on Chat Completions). Both adapters dropped it, so a refusal looked like an empty answer that ended normally. An answer the provider's safety system stopped (`content_filter`) or that hit the output limit (`length`) also ended silently: the finish reason was recorded on `session.step.ended.1` but no client showed it. Codex now explains safety blocks in its TUI; Cyber Code should at least show the refusal and say what happened.

## What Changes

- The OpenAI Responses and Chat Completions adapters deliver refusal text as answer text.
- Assistant messages carry `finish`, the step's finish reason, in the messages API.
- The TUI shows a warning under an answer that the safety filter stopped or that reached the output limit.
- `cyber exec` reports `stop_reason` `refusal` or `max_tokens` for such a final answer and, in text mode, prints a warning on stderr.

## Impact

- Specs: `provider-catalog`, `tui`, `exec-mode` (added requirements).
- Code: `cyber-llm` OpenAI adapters, `cyber-server` message model, `cyber-tui` transcript, `cyber exec` report; OpenAPI document and SDK.
- Docs: local core guide, CHANGELOG.
