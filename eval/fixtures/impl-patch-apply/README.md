# patch-apply

Applies unified diffs to an in-memory set of files (Node 22, ES modules, no dependencies).

```js
import { applyPatch } from "./src/applyPatch.js";

const files = { "greeting.txt": "hello\nworld\n" };
const result = applyPatch(files, patchText); // { ok: true, report: [...] } or { ok: false, error }
```

`SPEC.md` is the authoritative contract: the patch format accepted, how hunks are located
(exact context, offset search, ordering), the `reverse` and `maxOffset` options, atomicity, the
result shape and every error message. `src/applyPatch.js` is still a stub.

Run the tests with `node --test`.
