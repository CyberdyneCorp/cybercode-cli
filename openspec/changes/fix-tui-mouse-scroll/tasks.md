## 1. Mouse capture

- [x] 1.1 Enable mouse capture in fullscreen setup and after an editor suspend, and release it on teardown; follow `tui.mouse` while running (`runner::mouse_tests::capture_turns_on_the_terminal_mouse_reports_and_off_again`).
- [x] 1.2 Route wheel events: transcript by 3 lines, Up/Down in scrolling and list panels, ignored by prompts and confirmations; clicks unused (`mouse::tests`, `runner::mouse_tests::a_wheel_event_from_the_terminal_scrolls_the_transcript`).
- [x] 1.3 Add the `mouse` preference (`on`/`off`, default `on`) and document it in docs/local-core-guide.md.
