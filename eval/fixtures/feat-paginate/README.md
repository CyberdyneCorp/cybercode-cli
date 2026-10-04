# products

Product search helpers (Node 22, ES modules, no dependencies). `searchProducts` in
`src/products.js` returns every match, and the catalog is now large enough that the HTTP layer
needs to return results one page at a time. The pagination helper belongs in `src/paginate.js`
(see the task description for its exact contract).

Run the tests with `node --test`.
