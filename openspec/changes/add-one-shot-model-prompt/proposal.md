## Why

Sometimes one question should go to a different model (a stronger reasoner, a cheaper one) without changing the Session. Switching with `/model`, asking, and switching back takes three steps and is easy to forget. The runtime already runs a command's Turns on the command's `model` and then returns to the Session's model, but a plain prompt could not ask for that.

## What Changes

- `session-runtime` "Agent, model, and mode switching": `POST /sessions/{id}/prompt` accepts `model`. That prompt's Turns, tool loop included, run on it until the final text reply. The Session's model and its switch events are unchanged. An unparseable or unresolvable model is refused before admission, and the prompt's `model` takes precedence over a command's. The inbox row reports `model`, and the exact-retry digest includes it.
- `tui` "Model, variant and agent selection": `/model <provider/model> <prompt>` sends the prompt with that `model`, queued while the Session runs. `/model` and `/model <name>` open the picker as before.

## Impact

- `crates/cyber-server`: `PromptBody.model`, `Admission.model`, `Admitted.model`, `InboxRow.model`, and admission validation; the command-model selection generalized to `InboxRow::turn_model`.
- `crates/cyber-tui`: the `/model` arguments and `Action::ModelPrompt`.
- OpenAPI document and TypeScript SDK regenerated.
