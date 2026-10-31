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

The initial control injected zero with stale EPERM into a copied full IPC matrix,
then required a second matrix with the old guards to falsely pass. That design
coupled a deterministic send-result regression to unrelated concurrent alias
lookups. The corrected control compiles the exact shared send/result helpers in
`macos_datagram_send_test.c`, with only sendmsg replaced by an explicit fault
injector. It is verifier regression evidence, not native syscall or confinement
acceptance. The normal nine-case IPC matrix still performs all real concurrent
host/private alias replacements and retains every existing assertion.

Six deterministic cases cover zero with stale EPERM, short with stale EACCES,
actual -1 permission results, -1/EINVAL, and setup refusal before any send. They
also verify first-failure preservation. The shared helper reports ENAMETOOLONG
for an oversized socket path instead of leaving stale errno. Only -1 plus a
permission error can count as refusal; nonnegative and unknown results must fail.

A copied shared header restores both old guards: unconditional errno capture and
permission classification without an attempted -1 send. The same compiled test
must execute and fail specifically at `zero-stale-EPERM`, showing raw zero and
outcome D. Compiler/startup failure cannot satisfy this control. The workflow
retains both logs and a source-bound report that labels the legacy classification
invalid. It does not demand that an unrelated native race falsely pass.

## Hosted failure at 3f7067dd

The [Intel job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41173821688/job/124457138622)
passed all nine primary IPC rows, then correctly rejected the injected zero send
at `r0w1s1`. Its legacy comparison failed earlier at the unmodified `r0w0s1` race:
raw return -1, errno 22, seven private acknowledgements and eight classifications
as refusals in sixteen attempts. The thread settled and host receipts were zero.
This new raw-return witness establishes an actual failed send in that copied
comparison run. It does not establish the cause, nor prove that an earlier
historical report without a raw result had the same cause.

The failed job remains failed evidence, retained under
`scripts/probes/plugin-confinement/evidence/macos-send-result-intel-3f7067dd`.
The archive digest, actual tested merge/head tree equality, and normal IPC
manifest were independently verified. The deterministic regression removes the
unrelated-race dependency; it does not fix or accept an unknown native race
outcome. Unknown send errors still fail the full normal matrix without retries.

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

The initial local arm64/Darwin checks and copied-matrix counterexample remain
historical evidence under `scripts/probes/plugin-confinement/evidence/macos-send-result-local`.
After isolating the regression, all eight exact native-signing workflow steps
pass locally, including all nine real IPC rows, the native capability matrix,
signature/exec controls, missing scratch/scope/rights/path controls, and the six
send-result cases with their precise missing-guard failure. Current-source
reports/logs are retained under
`scripts/probes/plugin-confinement/evidence/macos-send-result-deterministic-local`;
the actual source bytes were independently matched to every report manifest.
Current-source hosted/runtime/platform acceptance remains required; neither a
passing diagnostic nor this verifier correction completes production enforcement.

## Review and checks

The change is confined to diagnostic fixtures, their verifier/workflow and
evidence/documentation; production launch policy is unchanged. Review covered
errno lifetime, attempted-send qualification, first-failure preservation,
nullable JSON output, cleanup and control specificity. No blocking code or
security findings remain in this diff. The injected copies remain outside the
source checkout, and only the corrected normal reports count as positive evidence.

Verification: all nine local normal IPC rows; all eight native-signing workflow steps,
including the zero/short/unknown/setup cases and its deliberately failing
legacy-guard counterexample; native capability/signature/exec setup; YAML, shell and embedded
Python syntax; `git diff --check`; strict OpenSpec validation (114 passed, zero
failed); spec lint (zero errors, 25 warnings). The evidence report source hashes
were independently recomputed against the actual fixture bytes. No Rust tests
were run for this probe-only change. Hosted macOS arm64/x64 and the configured
Linux checks have not yet been run on this revision.
