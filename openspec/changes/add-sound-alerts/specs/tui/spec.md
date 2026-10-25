## ADDED Requirements

### Requirement: Sound alerts
(P1) The TUI SHALL play a short sound for each alert event:
- `input`: a permission or question request is waiting;
- `done`: the Session finished a Drain and is idle, or the Session sent a notification (`session.notification.sent`);
- `error`: a Turn failed or `session.error` arrived;
- `workflow`: a watched workflow run completed;
- `workflow_failed`: a watched workflow run ended any other way: failed, stopped, over its budget, or paused by a restart;
- `message`: a cross-session message was delivered to or held for the attached Session.

`tui.sounds.when` SHALL be `unfocused` (default: play only while the terminal reports it is unfocused) or `always`.

At most one sound SHALL play every 1.5 seconds. An alert of higher priority, in the order `input`, `error`, `workflow_failed`, `done`, `workflow`, `message`, SHALL replace a waiting lower sound or cut a playing lower one short, and other alerts inside the window SHALL be dropped.

Playing a sound SHALL never block drawing or key handling. Sounds SHALL be played by the platform's own player, run directly and never through a shell:
- `afplay` on macOS;
- `pw-play`, `paplay` or `aplay` on Linux;
- `System.Media.SoundPlayer` through PowerShell on Windows, with the file path passed in an environment variable.

Each player process SHALL have no access to the terminal. It SHALL be killed after 5 seconds, when a newer sound starts, and when the TUI exits, including on SIGHUP and SIGTERM. A player SHALL only be searched for in absolute `PATH` entries.

The system SHALL fall back to the terminal bell when:
- the pack is `bell`;
- no player is found, or the player cannot be started;
- `SSH_CONNECTION` or `SSH_TTY` is set and `tui.sounds.play_over_ssh` is not `true`.

A player that exits with an error SHALL ring the bell for that alert and SHALL be kept for later alerts.

When a sound plays, the bell that `tui.notifications: auto` would write SHALL be left out. The desktop notification SHALL still be sent. At volume 0 no sound SHALL play, and the notification SHALL keep its bell.

`tui.sounds.enabled` SHALL default to `true`, and `tui.sounds.events.<event>: false` SHALL silence one event. The server and `cyber exec` SHALL never play sounds.

#### Scenario: Approval needed while away
- **WHEN** the terminal is unfocused, sounds use the defaults, and the attached Session asks for permission to run `npm test`
- **THEN** the `cyber` pack's `input` sound plays once through the detected player, an OSC 9 notification is written, and no BEL is written

#### Scenario: Burst coalesces
- **WHEN** the terminal is unfocused and a Session goes idle and a message arrives 200 ms apart
- **THEN** only the `done` sound plays

#### Scenario: Focused terminal stays quiet
- **WHEN** the terminal is focused, `tui.sounds.when` is `unfocused`, and a Turn finishes
- **THEN** no sound plays

#### Scenario: SSH session uses the bell
- **WHEN** `SSH_CONNECTION` is set, `play_over_ssh` is unset, and an `error` alert fires while unfocused
- **THEN** no player is started and a BEL is written to the terminal

#### Scenario: No player available
- **WHEN** no supported player is on `PATH` and a `done` alert fires while unfocused
- **THEN** a BEL is written, and one `info` log line names the reason

### Requirement: Sound packs
(P1) The system SHALL ship four packs:
- `cyber` (default): FM chimes;
- `soft`: sine tones;
- `retro`: square-wave arpeggios;
- `bell`: the terminal bell only.

The packs other than `bell` SHALL have a different sound for each of the six events. Their sounds SHALL be synthesized in code, with no audio files bundled, as mono 16-bit 44.1 kHz WAV. Each sound SHALL:
- last at most 0.9 seconds;
- peak at or below −1 dBFS;
- start and end below −40 dBFS over its first and last 2 ms;
- be byte-identical across runs for the same pack, event, volume and synthesizer version.

Rendered files SHALL be cached under the cache directory, keyed by pack, event, synthesizer version and volume, and written atomically.

`tui.sounds.volume` (0–100, default 60) SHALL scale the rendered samples. `tui.sounds.events.<event>` SHALL also accept:
- a pack name, to use that pack's sound for the event;
- an absolute or `~/` path to a regular file of at most 5 MiB with the extension `.wav`, `.aiff`, `.aif`, `.mp3`, `.ogg` or `.oga`.

A path SHALL be honoured only from the global `tui.jsonc`. A rejected or missing file, a file the detected player cannot decode (`aplay` and PowerShell take only `.wav`, `afplay` takes no Ogg), or a file the player fails to play SHALL fall back to the pack's sound with one warning.

A project `tui.jsonc` SHALL only be able to set `sounds.enabled: false`, lower `sounds.volume`, or set an event to `false`. Any other `sounds` value from a project SHALL be ignored with a warning.

#### Scenario: Deterministic, click-free sounds
- **WHEN** the `retro` `workflow` sound is rendered twice at volume 60
- **THEN** both files are byte-identical, the peak is at most −1 dBFS, and the first and last 2 ms are below −40 dBFS

#### Scenario: Project cannot pick a file
- **WHEN** a project `tui.jsonc` sets `sounds.events.input` to `/tmp/x.wav` and `sounds.enabled` to `true` while the global file has `enabled: false`
- **THEN** sounds stay off, the project values are ignored, and a warning names the ignored keys

#### Scenario: Custom file from global config
- **WHEN** the global `tui.jsonc` sets `sounds.events.done` to `~/sounds/done.wav`, a 40 KiB regular file
- **THEN** a `done` alert plays that file, and the other events keep the pack sounds

### Requirement: Sound settings command
(P1) `/sounds` (also in the command palette, binding `sounds_view`, unbound by default) SHALL open an overlay that:
- turns sounds on and off;
- picks the pack;
- changes the volume in steps of 10;
- sets `when`;
- turns each event on and off;
- previews each event's sound.

A preview SHALL play regardless of focus, `when` and the throttle. The overlay SHALL show the detected player, or the reason the bell fallback is used. Choices SHALL be saved to `kv.json`, where they override the global `tui.jsonc` unless that file was written after them (as `/theme`), and are overridden by the restrictions of a project `tui.jsonc`. Turning an event back on SHALL restore the pack or file the global `tui.jsonc` gives it. Every state SHALL be shown as a word, so the overlay is usable with `NO_COLOR` and in screen-reader mode.

#### Scenario: Pick a pack and preview
- **WHEN** the user opens `/sounds`, selects `retro`, and presses `p` on the `input` row
- **THEN** the `retro` `input` sound plays, and `retro` is still selected after the TUI restarts

#### Scenario: Fallback shown
- **WHEN** `/sounds` is opened over SSH without `play_over_ssh`
- **THEN** the overlay shows `bell only: SSH session`

### Requirement: Needs-input attention
(P0) When the attached Session gains a permission or question request, or one is routed to it, that this TUI has not alerted for, the TUI SHALL fire the attention notification of `tui.notifications` with the text `cyber: <session name> needs input`, together with the `input` sound alert. This SHALL happen once per request ID per TUI process, including when a reconnect or snapshot shows the same request again, and it SHALL follow the same focus rule as the `input` sound.

#### Scenario: Permission request notifies once
- **WHEN** the terminal is unfocused and a permission request appears, then the event stream reconnects and the request is still pending
- **THEN** exactly one notification `cyber: <session name> needs input` and one `input` sound are produced
