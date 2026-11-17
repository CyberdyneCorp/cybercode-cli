## 1. Fix

- [x] 1.1 Redact and bound a hook's reason, stop reason and diagnostic with the Location's redactor after the handler runs, before merge, notices and the tool result.
- [x] 1.2 Redact and bound the decision stored in a settled Session hook receipt.
- [x] 1.3 Redact and bound every hook notice built from hook output (synchronous and `async` handlers).

## 2. Verification

- [x] 2.1 Regression test: an exit-2 hook printing a token leaves it out of the tool result, the next model request, the Session events, live notices and the receipt, and keeps the rest of the reason.
- [x] 2.2 Regression test: a 200 KB block reason reaches the tool result and the receipt bounded.
- [x] 2.3 Regression test: an exit-1 hook's stderr notice is redacted.
- [x] 2.4 Unit tests: redaction before the bound, a cut on a character boundary, the rewrite kept.

## 3. Docs

- [x] 3.1 Hooks section of the local core guide.
