## 1. Windows shell resolution

- [x] 1.1 Resolve the `bash` tool shell on Windows from `PATH` and Git for Windows roots (from `git.exe` on `PATH` and `ProgramFiles`, `ProgramW6432`, `ProgramFiles(x86)`, `LOCALAPPDATA\Programs`), skipping the WSL launcher directories and checkout-planted binaries; an explicit absolute path wins; no shell is a clear error (`crates/cyber-tools/src/tools/shell_resolve.rs`, `tools/bash.rs`).
- [x] 1.2 Unit tests on every platform for the launcher exclusion, Git-on-`PATH` discovery (`cmd`, `bin`, `mingw64\bin`), checkout and relative entries, and the explicit path; a native Windows test that bare and POSIX defaults resolve to an absolute non-launcher shell (Windows CI step "Native Bash shell selection").

## 2. Archive

- [ ] 2.1 Archive this change.
