# Private PID namespace descendant-settlement fixture

This tests the lifetime requirement of the existing [Linux broker candidate](linux-socket-broker-candidate.md).
It does not activate native hosting, select a support floor or replace production
resource/generation admission.

With shell enabled, the fixed C target forks a descendant that successfully calls
setsid. The descendant checks that its stdin is the controlled blocking pipe,
flushes a stdout receipt, and acknowledges readiness over an internal pipe. Only
then does the plugin leader exit. The descendant retains standard input/output
and does not exit voluntarily while the driver holds its input open.

The driver requires the owned bwrap process to exit successfully and stdout to
reach EOF before it closes that input or signals anything. It keeps reading for
up to ten seconds after owner exit, within its overall 30-second deadline. This
is an observed closure condition, not a fixed sleep considered successful cleanup.
The fixture establishes a live descendant in another session; original-process-
group termination alone cannot settle it. The kernel's
[PID namespace documentation](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html)
describes termination of remaining namespace processes when its init exits;
native execution must establish how the actual composed bwrap launch behaves.

The precise control removes only the private PID namespace. It replaces the
private proc mount with a read-only bind of proc so lack of mount permission does
not masquerade as the expected counterexample. All other composition remains.
It must show successful owner exit and a descendant in its own session retaining
stdout before cleanup. A startup failure, failed setsid, uncontrolled stdin or
owner that never exited cannot count as that counterexample.

The controlled descendant remains safely releasable without PID lookup/signals:
after capturing the pre-cleanup facts, the owner closes its exact input pipe.
The fixture's only remaining path reads EOF and exits, closing output. Both reader
threads are drained before any assertion and fixture-path cleanup. The normal
private-namespace case must settle before this fallback release. This fallback
is specific to the fixed source-bound fixture, not a termination protocol for
arbitrary plugins.

No production composer/helper or ordinary tool behavior changes. The positive
case runs across all four read/write combinations with shell true. Other signal,
fork/exec, nested namespace, saturation and owner/admission obligations remain
separately gated. Local Rust example Clippy, portable C syntax, workflow syntax,
formatting/diff and OpenSpec can run on macOS; the Linux branch, positive native
settlement and missing-namespace control require hosted x64/arm64 execution.
