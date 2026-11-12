## Why

A pre-release security review of the code found four high and three medium issues that a cloned repository, a prompt-injected model or a tampered model catalog could exploit:

- **H1:** a repository's skill could pre-approve tools through `allowed-tools` without the checkout being trusted.
- **H2:** file tools checked the path as typed and then followed a symbolic link, so a committed or dangling link reached protected or outside files.
- **H3:** a redirection with a file descriptor (`1>>`) was invisible to the shell analysis.
- **H4:** the downloaded catalog chose which host received a provider's key.
- **M1:** provider clients followed redirects, carrying key headers across hosts.
- **M2:** the read-only classifier accepted environment assignments, wrappers and writing options, and "always" approvals of code-running programs were wildcards.
- **M3:** the protected-path list missed startup files and Cyber Code's own state, and compared case-sensitively on macOS and Windows.

## What Changes

- `skills-commands` / `workspace-trust`: repository skills' `allowed-tools` count only in a trusted checkout; granting skills are part of the trust digest; `skills.paths` is held until trusted.
- `builtin-tools`:
  - new "Symbolic links in file tools": file tools check and act on the link's target;
  - "Bash permission analysis": redirections with descriptors are writes, and code-running programs get exact "always" patterns.
- `permissions-modes`:
  - "Protected paths": more startup and credential files, Cyber Code's state and data, relocated directories, case-insensitive on macOS and Windows;
  - new "Read-only shell classification".
- `provider-catalog`: new "Catalog source integrity" (pinned first-party endpoints and key variables, HTTPS or loopback only).
- `provider-credentials`: new "No credentials across redirects".

## Non-goals

The review's low findings (query-token routes, CORS origins read from project config, `--attach` over HTTP, temp-file and log-file modes, `install.sh` checksum-only fallback) are left for a follow-up.

## Impact

- `crates/cyber-tools`: skills, file tools, bash analysis, taint classifier, protected paths.
- `crates/cyber-core`: config gate, skill trust subset, trust digest.
- `crates/cyber-llm`: catalog parsing and provider client redirects.
- Regression tests for each finding.
