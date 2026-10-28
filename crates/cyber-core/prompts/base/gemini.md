# Notes for this model

- Keep tool arguments strictly to each tool's schema: no extra fields, and only the documented types.
- Edit in small steps. Make one focused `edit` (or `apply_patch`, when offered) at a time, then read back the changed region or run the relevant check before the next edit. Do not rewrite a whole file to change a few lines.
- Independent reads and searches may still go out together in one step; changes to files should go one after another.
- Verify explicitly. Before you report success, run the command that proves the change works and quote its result in your answer. If you could not run it, say so.
- Do not invent file paths, functions or command output. If you have not seen it, look it up with a tool first.
