## Why

On Windows the `bash` tool used its configured shell name as given: the default `/bin/sh` (or `$SHELL`, usually unset) does not exist, and a bare `bash` is found by `CreateProcess` as `C:\Windows\System32\bash.exe`, the WSL launcher. Commands failed, or ran inside a WSL distribution instead of the Location's Windows environment.

## What Changes

- `builtin-tools` "bash tool": on Windows, a configured shell that is not an existing absolute path to a supported shell is resolved by its name (`bash`, `sh`, `zsh`, `dash`; anything else as `bash`) from `PATH` and Git for Windows installs, never from the WSL launcher directories (`System32`, `Sysnative`, `WindowsApps`) and never from inside the checkout. An explicitly configured absolute path is used as given, including the WSL launcher. No shell found fails the call with a message naming Git for Windows and the `shell` setting.
- Template shell injection and command hooks that use the `bash` shell follow the same resolution.

## Capabilities

`builtin-tools` only. No configuration or API change.
