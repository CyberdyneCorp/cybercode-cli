# tiny-mustache

A small logic-less template engine (a Mustache subset) for Node 22, ES modules, no
dependencies. `render(template, view, partials = {})` lives in `src/render.js`; SPEC.md is the
authoritative contract (tags, lookup, escaping, sections, standalone lines, partials, set
delimiters and exact error messages).

The engine is not written yet. Run the tests with `node --test`.
