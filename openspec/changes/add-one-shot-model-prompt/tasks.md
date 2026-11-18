## 1. Prompt model

- [x] 1.1 `model` on prompt requests, admissions, admitted events and inbox rows; validation before admission; the retry digest.
- [x] 1.2 Turn model selection through `InboxRow::turn_model` (prompt model, then command model), expiring at the final text reply.
- [x] 1.3 Runtime test through the tool host: tool loop on the prompt model, Session model unchanged, next prompt on the Session model, unknown model refused.
- [x] 1.4 TUI `/model <provider/model> <prompt>`; unit and real-server tests.
- [x] 1.5 OpenAPI, SDK, guide and CHANGELOG.
