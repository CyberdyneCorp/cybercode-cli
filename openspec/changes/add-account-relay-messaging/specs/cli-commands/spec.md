## ADDED Requirements

### Requirement: Machines command
(P3) `cyber machines` SHALL provide these subcommands for Relay machine identity and trust:
- `list` (alias `ls`): name, `mch_` ID, account or org member, trust state, online state and key fingerprint, as a table or with `--format json`.
- `whoami`: this machine's name, ID, fingerprint and Relay state.
- `rename <name>`: change this machine's name; approved machines are told over the encrypted channel.
- `approve <name|id>`: show the six-word code and ask for confirmation; `--yes` skips the prompt.
- `reject <name|id>`: refuse the machine and stop its prompts.
- `forget <name|id>`: delete trust and close channels at once.

Subcommands other than `whoami` SHALL fail with `AccountRequiredError` (exit 6) when no Cyber Account is signed in.

#### Scenario: Approve after comparing codes
- **WHEN** the user runs `cyber machines approve laptop` and confirms that the six words match the ones `laptop` shows
- **THEN** `laptop` is approved on this machine and `cyber machines list` shows it as `approved`, or as `awaiting peer` until `laptop` approves back
