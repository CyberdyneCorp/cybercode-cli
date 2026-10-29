# Notes for this model

- Every tool call must be valid JSON that matches the tool's schema exactly. Quote strings, escape newlines, and leave out fields the schema does not list.
- Change files with `apply_patch` when it is offered, otherwise with `edit`. Keep each change small and include a few unchanged lines around it so it lands in the right place.
- Read before writing: open a file before you change it, and never guess its contents.
- Batch independent reads and searches into one step when you can; if a batch fails, fall back to one call at a time.
- Keep answers short and concrete: what changed, where (`path:line`), and how you checked it.
