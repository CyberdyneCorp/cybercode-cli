## MODIFIED Requirements

### Requirement: powershell tool
(P1) On Windows, `powershell` SHALL run commands natively in `pwsh`, falling back to `powershell.exe`, with the same parameters, limits, sandboxing and permission analysis as `bash`. The permission action SHALL be `bash` so a single ruleset covers both.

#### Scenario: PowerShell on Windows
- **WHEN** the server runs on Windows and `pwsh` is installed
- **THEN** `powershell` is advertised and its calls are checked against `bash` rules

#### Scenario: Native PowerShell permission resources
- **WHEN** native PowerShell source contains multiple simple commands, literal filesystem operands or output redirections
- **THEN** permission resources SHALL come from the bounded PowerShell AST rather than a Bash parse
- **AND** ordinary quoted output SHALL remain data
- **AND** literal filesystem targets SHALL participate in external-directory and protected-path checks
- **AND** unresolved bindings or command contexts SHALL NOT enable classifier approval

#### Scenario: Native PowerShell foreground lifecycle
- **WHEN** an approved native PowerShell call times out or its Turn is interrupted
- **THEN** its parent-owned process boundary SHALL terminate the interpreter and settle the call
- **AND** closed standard input SHALL remain closed to the user command
- **AND** unavailable Windows confinement or a missing process-owner helper SHALL refuse launch

#### Scenario: Native PowerShell discovery and modes
- **WHEN** Windows tool definitions are requested
- **THEN** interpreter discovery SHALL prefer an installed absolute `pwsh.exe` before `powershell.exe` and exclude checkout executables and relative PATH entries
- **AND** plan mode or a `bash` tool deny SHALL hide `powershell`
- **AND** a denied constituent command SHALL refuse compound source execution
