# Plugin image admission experiment

Run on macOS with Node 22 and command-line developer tools:

```sh
node --experimental-strip-types scripts/probes/plugin-admission/run.mts --report /tmp/plugin-admission.json
```

This is an admission experiment using dummy files, not production enforcement.
It reuses the native pre-exec launcher and requires normal target startup.

The first control demonstrates two gaps in a path-only policy: a pre-existing
hard link in the readable package exposes a private file, and an existing link
in writable scratch modifies the original read-only package file. Preventing
link creation after confinement starts does not cover those existing aliases.

A small stat-only guard refuses known private files or staged images whose link
count is not one. The target's independent startup receipt must remain absent.
The driver then copies package data and the native executable onto distinct
inodes, preserves the executable's bytes/signature, changes the original through
its existing alias, and verifies that the copies remain intact. Adding a writable
alias to the staged executable must also refuse startup. With unsafe aliases
removed, the copied executable must start and the OS must deny direct private
reads and staged-package writes.

The guard inventories only named fixture files. Complete private-root coverage,
directory bindings, concurrent replacement, generation ownership, bounded copying,
helper distribution, Linux/Windows behavior and actual host/SQLite integration
remain unproved. A successful report includes an observed bypass and targeted
refusals; it cannot be counted as production capability enforcement.

The independent workflow runs on macOS arm64/Intel and does not restart the
separate runtime capability workflow when only this admission experiment changes.
