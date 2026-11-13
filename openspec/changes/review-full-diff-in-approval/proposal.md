## Why

The permission prompt showed only the first 12 lines of an edit's diff, so a large edit could not be reviewed before approving it. Codex added inline patch review to its approval prompts (openai/codex#58281); this is the clean-room counterpart.

## What Changes

- The prompt shows the whole diff.
- When the details do not fit, they scroll with PgDn, PgUp and Home, as the blocked-call approval already does, while the choices stay in view under a hint naming those keys.
- Prompts that fit look as before.

## Impact

- Spec: `tui` (Permission prompt).
- Code: `cyber-tui` permission overlay and its keys.
