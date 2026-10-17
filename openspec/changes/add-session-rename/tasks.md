## 1. Rename

- [x] 1.1 Clean user-set titles in `Runtime::rename` and refuse empty ones; regression tests for the HTTP route and for a generated title arriving after a rename.
- [x] 1.2 TUI `/rename [title]`, its dialog, the palette action, the `session_rename` binding, `/help`, and `session.renamed.1` updating the header and sidebar; a screens test against a production App.
- [x] 1.3 `cyber sessions list|show|rename|delete` and the shared ID/title/prefix resolver for `cyber -r` and `cyber exec --session`; real-binary tests for listing, renaming, resuming by title and the ambiguous case.
- [ ] 1.4 Page `cyber sessions list` through `$PAGER` on a TTY ("Table paging" scenario).
- [ ] 1.5 `cyber sessions export|import|fork|rewind|repair-context|share|unshare`.
