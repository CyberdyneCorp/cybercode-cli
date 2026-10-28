# Notes for this model

- Make file changes with the `edit` tool: replace one exact, unique span of text at a time, copied from the file as it stands now. Read the file first so the span matches byte for byte. Use `write` for a new file or a full rewrite you have already read.
- Use tools in parallel. Whenever calls do not depend on each other's results, such as reading several files, running independent searches, or checking status, put them all in a single response.
- When a tool result changes your plan, say so in one line before you continue.
- Keep the final answer short. Prefer prose and a few bullets over headings.
