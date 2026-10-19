## 1. Projection and events

- [ ] 1.1 Message tree in the Session model (`parent_message_id`, `active_tip`); a committed conversation revert moves the tip instead of truncating; replay test that a pre-change rewind yields two branches with the same active path.
- [ ] 1.2 `session.branch.checked_out.1` and `session.branch.labeled.1` (label grammar, uniqueness); idle check with `SessionBusyError`; `exact` and newest-tip selection.
- [ ] 1.3 Optional code restore on checkout through the three-phase revert; tests that user edits are preserved.

## 2. Context and compaction

- [ ] 2.1 Context assembly from the active path; epoch summaries reused only for ancestor prefixes; test that a sibling's summary never reaches the model.

## 3. Surfaces

- [ ] 3.1 Routes `GET .../tree`, `POST .../checkout`, `PUT|DELETE .../labels/:name`, `GET .../messages?branches=all`; OpenAPI and regenerated SDK.
- [ ] 3.2 `cyber sessions tree|checkout|label` with ID/prefix/label resolution; real-binary tests.
- [ ] 3.3 TUI `/tree`, `/label`, the branch header, palette entries and `/help`; screen tests against a production App.
- [ ] 3.4 `cyber sessions export --all-branches`.

## 4. Docs and verification

- [ ] 4.1 Local core guide section on branches; README Sessions row.
- [ ] 4.2 `cargo fmt`, `cargo clippy --workspace -- -D warnings`, touched-crate tests, SDK checks, `openspec validate --strict`.
