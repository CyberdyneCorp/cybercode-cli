## ADDED Requirements

### Requirement: Named sandbox profiles
(P1) `sandbox.profiles.<name>` SHALL define `{ extends?, policy?, writable_roots?, readable_paths?, deny_read?, network?: { mode, allowed_domains?, denied_domains? }, env? }`, where `extends` names another profile and lists merge (deny lists union). The built-in profiles `read-only`, `workspace` and `full` SHALL correspond to the three policies. `--sandbox <policy|profile>`, `sandbox.profile`, agent `sandbox` and org policy `sandbox.min_profile` SHALL select a profile. `deny_read` globs SHALL be enforced by the OS sandbox and SHALL also hide matching paths from `read`, `glob` and `grep` results. `cyber sandbox explain --profile <name>` SHALL print the resolved profile.

#### Scenario: Profile hides secrets
- **WHEN** profile `ci` extends `workspace` with `deny_read: ["**/*.pem", "~/.aws/**"]` and a sandboxed command runs `cat key.pem`
- **THEN** the read is denied by the sandbox and `glob` does not list `key.pem`

### Requirement: Environment policy
(P1) `sandbox.env` SHALL accept `mode` (`inherit`, default: the server's environment filtered by the credential-masking rules; or `none`: only `PATH`, `HOME`, `TMPDIR`, `LANG`, `TERM` and `set` values), `set` (a map of variables added to every sandboxed process), `allow` and `deny` (glob patterns applied after masking) and `login_shell` (default false; when true, `bash -l` is used so profile scripts run). The resolved environment SHALL be shown by `cyber sandbox explain`.

#### Scenario: Clean environment for CI
- **WHEN** `sandbox.env.mode` is `none` and `set` is `{ "CI": "1" }`
- **THEN** a sandboxed `env` prints only the five base variables plus `CI=1` and the `CYBER_*` markers
