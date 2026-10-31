# Native datagram send-result validation

This corrects the native measurement contract. It does not activate the App's
native launcher, approve the Linux dependency revision or close parent tasks.

## Reproduced defect

The earlier datagram helper reduced the send return value to a boolean. Its
caller then inspected errno whenever the expected twelve bytes were not sent.
A nonnegative short result could therefore inherit an unrelated permission errno
and be counted as a policy refusal. A copied fixture returning zero with stale
EPERM for the r0w1s1 race's sendmsg calls passed all nine cases before this fix,
including sixteen outcomes classified as refusals in that case. Those were not
sixteen demonstrated failed native sends.

The [Apple send manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/send.2.html)
distinguishes a byte count from a -1 failure accompanied by errno. The corrected
helper preserves the raw result, captures errno immediately only after -1, and
requires both an attempted syscall and -1 plus EACCES/EPERM for a refusal.
An invalid setup cannot manufacture a send attempt. Cleanup preserves a socket
setup error instead of rereading errno after close.

The first unexpected send or private reply retains `failure_return` and its
error code. A nonnegative short send is `send_size` with error code zero; it
fails the existing acceptance assertion. A -1 error outside the two permission
codes still fails as `send`. `failure_return` is null when no unexpected send or
reply result was captured, including successful rows and failures in other
stages. No error is retried or silently recategorized as a successful exchange.

## Regression and missing-guard control

A copied fixture substitutes zero with stale EPERM only for sendmsg through the
r0w1s1 race alias. Its other exchanges, unrestricted control and private positive
control remain real. The corrected driver must execute and fail the named
mutation assertion on attempt two, with `send_size`, raw return zero, error zero,
a settled thread and zero owner host receipts. Compilation or startup failure
cannot satisfy the control.

A separate copy restores the old unconditional errno capture and removes the
raw-return qualification. That copy must falsely pass all nine cases, including
at least eight refusal counts in the injected case. This demonstrates that the
fixture does not reject the injected condition for an unrelated reason. Its
report is labeled as an invalid legacy-guard counterexample, never acceptance.
Both copies and the main run still use source-bound reports.

## Evidence and remaining work

The historical d689d2f3 run completed 68 checks successfully with one intentional
historical skip. Its arm64/x64 native signing reports are retained under
`scripts/probes/plugin-confinement/evidence/macos-owned-unix-d689d2f3`. Archive
checksums, actual checkout/tree equality and each declared IPC source byte were
independently verified. These reports predate the raw-result fix; their refusal
classification does not resolve the earlier errno ambiguity.

The earlier Intel cf695982 report records errno 22, but lacks the raw syscall
result. It cannot establish that send returned -1/EINVAL rather than a short
nonnegative result with stale errno. The cause of that historical row remains
unresolved. Fresh native results must use the corrected raw-result witness.

Local arm64/Darwin checks pass all nine normal rows, the six exact workflow steps
and the original native capability/signature/exec setup. The zero-result control
fails precisely and its legacy-guard copy reproduces false acceptance. Evidence
is retained under
`scripts/probes/plugin-confinement/evidence/macos-send-result-local`.
Current-source hosted/runtime/platform acceptance remains required; neither a
passing diagnostic nor this verifier correction completes production enforcement.

## Review and checks

The change is confined to diagnostic fixtures, their verifier/workflow and
evidence/documentation; production launch policy is unchanged. Review covered
errno lifetime, attempted-send qualification, first-failure preservation,
nullable JSON output, cleanup and control specificity. No blocking code or
security findings remain in this diff. The injected copies remain outside the
source checkout, and only the corrected normal reports count as positive evidence.

Verification: all nine local normal IPC rows; all six IPC workflow steps,
including the deliberately failing zero-send case and its invalid legacy-guard
counterexample; native capability/signature/exec setup; YAML, shell and embedded
Python syntax; `git diff --check`; strict OpenSpec validation (114 passed, zero
failed); spec lint (zero errors, 25 warnings). The evidence report source hashes
were independently recomputed against the actual fixture bytes. No Rust tests
were run for this probe-only change. Hosted macOS arm64/x64 and the configured
Linux checks have not yet been run on this revision.
