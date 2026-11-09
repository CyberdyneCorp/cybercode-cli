## ADDED Requirements

### Requirement: Symbolic links in file tools
(P0) `read`, `write`, `edit`, `apply_patch`, `notebook_edit` and file delivery SHALL resolve every symbolic link in their path, including a dangling last link, before any check, and SHALL apply the external-directory, protected-path, permission and hidden-path checks to the resolved target, which is also what they read or write. A target inside the Location reached through a linked Location (such as `/var` → `/private/var`) SHALL keep the Location's spelling, so Location-relative rules still match. Deleting a file SHALL resolve only its directories and remove the entry itself.

#### Scenario: Committed link into .git
- **WHEN** a repository commits `notes.md -> .git/config` and an `accept-edits` Session writes `notes.md`
- **THEN** the write is checked as `.git/config`, the protected-path rule asks, and nothing is written without approval

#### Scenario: Dangling link out of the Location
- **WHEN** `settings.txt` is a dangling link to a file outside the Location and the model writes it
- **THEN** an `external_directory` request for the target's directory is raised first

## MODIFIED Requirements

### Requirement: Bash permission analysis
(P0) The system SHALL parse every command with tree-sitter (bash, or PowerShell for `powershell`). It SHALL evaluate the `bash` permission once per simple command, using its source text as resource, and offer an "always" pattern of the command's arity prefix plus ` *` (for example `git commit *`, `npm run *`). For programs that run code or edit files through their arguments (interpreters such as `python` or `node`, shells, wrappers such as `env`, `sudo`, `xargs` and `timeout`, and `find`, `sed` and `awk`) the "always" pattern SHALL be the exact command. A redirection SHALL count as a write whatever its file descriptor (`>`, `1>>`, `2>`, `&>`, `>|`); duplicating or closing a descriptor (`2>&1`, `>&-`) writes nothing. Path arguments of file-mutating commands (`rm`, `mv`, `cp`, `mkdir`, `touch`, `chmod`, `chown`, `ln`, `tee`, redirections) that resolve outside the Location SHALL first raise `external_directory`. Unparseable commands SHALL be evaluated as one resource and never auto-allowed by prefix rules.

#### Scenario: Compound command evaluated per part
- **WHEN** the command is `npm test && git push`
- **THEN** `npm test` and `git push` are evaluated separately and a deny on `git push *` blocks the call

#### Scenario: Descriptor redirection into .git
- **WHEN** the command is `echo x 1>> .git/config`
- **THEN** `.git/config` is a mutated path, the protected-path rule asks, and the command is not read-only

#### Scenario: Exact approval for find
- **WHEN** the user chooses "always" for `find . -name '*.rs'`
- **THEN** the saved pattern is `find . -name '*.rs'`, which does not cover `find . -exec rm {} +`
