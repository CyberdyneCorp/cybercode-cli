## MODIFIED Requirements

### Requirement: Upgrade and uninstall commands
(P0) `cyber upgrade [version] [--method <m>] [--channel <c>] [--dry-run]` and `cyber uninstall [--keep-config] [--keep-data] [--dry-run] [--force]` SHALL behave as specified in `installation-upgrade`. Both SHALL accept `--format json`, printing the upgrade result (`from`, `to`, `method`, `action`) or the uninstall plan (each entry's kind, path, size and whether it was removed).

#### Scenario: Dry-run uninstall
- **WHEN** the user runs `cyber uninstall --dry-run`
- **THEN** the paths that would be removed are listed with sizes and nothing is deleted

#### Scenario: Upgrade on another channel
- **WHEN** the user runs `cyber upgrade --channel beta` on a stable `script` install
- **THEN** the latest version is read from the `beta` channel manifest and installed after verification
