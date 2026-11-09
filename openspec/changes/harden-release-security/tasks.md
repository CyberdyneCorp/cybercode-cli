## 1. Fixes

- [x] 1.1 H1: ignore untrusted repository skills' `allowed-tools`; granting skills in the trust digest; `skills.paths` gated.
- [x] 1.2 H2: resolve symbolic links before file tool checks (dangling links included), keeping the Location's spelling; delete removes the entry.
- [x] 1.3 H3: redirections with descriptors are writes; descriptor duplication is not.
- [x] 1.4 H4: pinned first-party endpoints and key variables; catalog URLs HTTPS or loopback only.
- [x] 1.5 M1: provider clients do not follow redirects.
- [x] 1.6 M2: stricter read-only classification; exact "always" patterns for code-running programs.
- [x] 1.7 M3: protected paths extended, relocated directories, case folding on macOS/Windows.

## 2. Verification

- [x] 2.1 A regression test per finding; the full workspace suite.

## 3. Later

- [ ] 3.1 The review's low findings.
