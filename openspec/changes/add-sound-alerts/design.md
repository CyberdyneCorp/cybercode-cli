## Context

Attention today is one function, `App::attention(text)` in `crates/cyber-tui/src/osc.rs`. It writes a BEL and/or an OSC 9 or OSC 777 sequence, as chosen by `tui.notifications` (`auto` | `bell` | `osc9` | `osc777` | `off`). It is called from three places:
- `session.idle` while unfocused (`app.rs`);
- a Session notification, `session.notification.sent` (`side_chat.rs`);
- a workflow run's end (`workflow_watch.rs`).

Focus comes from crossterm `FocusGained` and `FocusLost` (`runner.rs`, `App::focused`). Permission and question requests reach the TUI through snapshots (`model.rs`, `Request::Permission` and `Request::Question`) and nothing alerts on them. Session errors arrive as `session.error`.

No audio code or dependency exists in the workspace. The release builds eight targets, including `*-linux-musl` and `aarch64-pc-windows-msvc`. A native audio stack such as `cpal` or `rodio` needs ALSA headers on Linux, does not cross-compile cleanly to musl, and would add about 1 MB.

## Goals / Non-goals

**Goals:**
- Each kind of alert has a distinct sound you can recognise without looking.
- The sounds are pleasant and short (0.15–0.9 s) and never clip.
- No new native dependency, and nothing in the binary grows beyond a few KB of code.
- An alert never blocks the UI or a key.
- The behavior is safe over SSH.
- Needs-input requests alert, which they don't today.

**Non-goals:** see the proposal. In short: no web dashboard sounds, no OS notifier, no TTS, and the server never plays audio.

## Decisions

### Alert events and one dispatch point

`App::alert(event: Alert, text: &str)` replaces direct `attention` calls. The `Alert` values, the trigger for each, and its priority:

| `Alert` | Trigger | Priority |
|---|---|---|
| `Input` | a permission or question request of the attached Session, or routed to it, whose ID this TUI has not alerted for before | 1 (highest) |
| `Error` | `session.error`, or a Turn that ended `failed` | 2 |
| `WorkflowFailed` | a watched run ended any way but `completed`: `failed`, `stopped`, `budget_exceeded`, or paused by a restart (`workflow_watch.rs`) | 3 |
| `Done` | `session.idle` after a Drain, or a Session notification | 4 |
| `Workflow` | a watched run `completed` | 5 |
| `Message` | an incoming cross-session message, `delivered` or `held` | 6 |

`alert` does two things. It calls the existing `attention(text)`, so the desktop notification and bell behavior of `tui.notifications` is unchanged. It also passes the event to the `Sounds` controller. When a sound actually plays, the BEL that `auto` would add is dropped so the user does not hear two alerts; the OSC 9 is still sent.

A Session notification (`session.notification.sent`, from the model's `notify` tool) maps to `Done`, and is notified whatever the focus, as before. A side-chat (`/btw`) answer is shown in the overlay the user is looking at and does not alert.

Requests reach the TUI through the attached Session's snapshot, so a request alerts when it belongs to the attached Session or is routed to it. Requests of Sessions only watched through `/agents` or the sidebar are not fetched and do not alert; that is left to a later change.

What an alert writes is decided in one place, `Notifications::alert_sequence(text, outcome)`: the notification without its bell when a sound covers the alert, and exactly one BEL when the bell stands in for the sound.

Alerted request IDs are remembered for the TUI process, in a bounded set of 512. A reconnect that replays the same pending request therefore does not alert again.

### When a sound plays

`tui.sounds.when` controls this:
- `unfocused` (default): only while the terminal is unfocused, which is the same gate `attention` uses for idle;
- `always`.

Terminals that never report focus leave `focused == true`, so `unfocused` stays quiet there. The docs say so and point to `always`.

**Throttle:** at most one sound every 1.5 s.
- A higher-priority alert in the window replaces a waiting lower one, or cuts a playing lower one short and restarts the window. A needs-input request soon after a `message` sound therefore still sounds.
- Alerts of equal or lower priority in the window are dropped: the sound already chosen covers them, so their notification leaves the bell out.
- At volume 0 nothing plays and the alert counts as silent, so the notification keeps its bell.
- Dropped alerts are counted for tests only.

`/sounds` previews ignore both `when` and the throttle.

### Synthesis: sounds as code

`sound/synth.rs` renders mono, 16-bit, 44.1 kHz PCM from a small list of notes:
- each `Note` has a frequency, start, length, a waveform (sine, FM, square or triangle) and a gain;
- every note gets an ADSR envelope with attack ≥ 4 ms and release ≥ 20 ms, so it never clicks;
- the mix is normalized to a −3 dBFS peak and then scaled by the volume.

Each pack is a table of event → notes, so tuning is a data change.

| Event | `cyber` (FM chime, ratio 3.5, index decaying) | `soft` (sine + 2nd partial, marimba decay) | `retro` (square 25 % duty, 8-bit arp) |
|---|---|---|---|
| `input` | E6 → B6 → E6, 3 × 90 ms: a "question" figure | G5 → D6 | C6–E6–G6–C7 arpeggio, 40 ms steps, repeated twice |
| `done` | C6 → G6 rising fifth, 320 ms tail | C5 → E5 → G5 | C5–E5–G5–C6 up |
| `error` | A4 → E4 falling, slightly detuned | E4 → C4 | C4–G3 down with pitch drop |
| `workflow` | C6–E6–G6–C7 bright arpeggio | C5–G5–C6 | fanfare G5–C6–E6–G6 |
| `workflow_failed` | G4 → D4 → A3 falling | D4 → A3 | G4–D4–A3 down |
| `message` | single A6 sparkle with a short echo | single A5 | two 30 ms blips at E7 |

Every sound is ≤ 0.9 s.

The output is deterministic: no randomness, and phase is computed from the sample index. Tests can therefore pin a hash for each sound and check:
- the peak is ≤ −1 dBFS;
- the first and last 2 ms are below −40 dBFS (no click);
- the duration is within its bound;
- each pack's six sounds are pairwise distinct.

The WAV writer adds a 44-byte RIFF header.

### Cache files

Rendered files go to `<cache_dir>/cyber/sounds/<pack>-<event>-v<SYNTH_VERSION>-<volume>.wav`.
- They are written atomically: a temp file, then a rename.
- They are rendered lazily on first use, and the six files of the current pack are pre-rendered when `/sounds` opens.
- Volume is part of the file name, so every player gets the same loudness without needing a volume flag.
- `SYNTH_VERSION` is bumped whenever the tables change.
- Stale files from another version are removed when the directory is first written, best effort.

### Players

`sound/player.rs` finds the player once at startup by searching the absolute entries of `PATH`; a relative entry (`.`, `node_modules/.bin`) would resolve against the project checkout, which could ship its own "player". It never runs anything through a shell.

| OS | Order | Command |
|---|---|---|
| macOS | `afplay` | `afplay <file>` |
| Linux, BSD | `pw-play`, `paplay`, `aplay` | `pw-play <file>` · `paplay <file>` · `aplay -q <file>` |
| Windows | `powershell.exe` | `-NoProfile -NonInteractive -Command "(New-Object Media.SoundPlayer $env:CYBER_SOUND_FILE).PlaySync()"` |

On Windows the path goes in the environment variable `CYBER_SOUND_FILE` and is never interpolated into the command string, so a path cannot inject PowerShell.

How a player runs:
- It is spawned with stdin, stdout and stderr null, and in its own process group on Unix, so Ctrl+C in the TUI does not reach it and it cannot draw on the screen.
- A detached task waits up to 5 s, then kills it.
- At most one player runs at a time, and a new sound kills the previous one.
- When the TUI exits, a running player is killed. The player has its own process group and so does not get the terminal's SIGHUP; the event loop therefore treats SIGHUP and SIGTERM as a quit, so the exit path stops the player.

**Fallback to the terminal bell (BEL).** The bell is used when:
- no player is found;
- the player cannot be spawned, or a pack sound cannot be rendered;
- the pack is `bell`;
- it is a remote session: `SSH_CONNECTION` or `SSH_TTY` is set and `tui.sounds.play_over_ssh` is false (default).

For an SSH session, the BEL travels to the user's local terminal, which is the one that should beep. The first fallback for each reason is logged once at `info`.

### Custom sound files

`tui.sounds.events.<event>` takes one of:
- `true` (the default) or `false`;
- a pack name, to borrow one event's sound from another pack;
- an absolute path, or one starting with `~/`, to a user file.

A path is accepted only from the global `tui.jsonc`. It must be a regular file of at most 5 MiB with the extension `.wav`, `.aiff`, `.aif`, `.mp3`, `.ogg` or `.oga`. Whether the format plays is up to the player, with two known limits: `System.Media.SoundPlayer` and `aplay` take only WAV (`aplay` would play anything else as raw noise), and `afplay` has no Ogg decoder; such a file is never handed to that player. The TUI checks the file when the configuration loads and again just before playing. A file that is rejected, missing, not playable by the player, or that the player exits on with an error falls back to the pack's sound and gives one warning. A failing exit on a pack sound rings the bell for that alert, is logged once, and keeps the player: one busy device must not silence the rest of the session.

### Configuration and precedence

```jsonc
// tui.jsonc
"sounds": {
  "enabled": true,          // default true
  "pack": "cyber",          // cyber | soft | retro | bell
  "volume": 60,             // 0–100
  "when": "unfocused",      // unfocused | always
  "play_over_ssh": false,
  "events": { "message": false, "input": "~/sounds/ping.wav" }
}
```

The layers apply in this order:
1. defaults;
2. global `tui.jsonc`;
3. `/sounds` choices saved in `kv.json`, which win over the global file unless it was written after them, the same way `/theme` works;
4. the project `tui.jsonc`, which can only restrict.

A project layer may set:
- `enabled: false`;
- a lower `volume`;
- an event to `false`.

Any other `sounds` key in a project file, and a project volume above the global one, is ignored with a warning. An unknown key or an out-of-range value gives a warning and keeps the default, matching the other `tui.*` keys. `tui.notifications: "off"` does **not** turn sounds off: the two are independent channels.

**Default on (confirmed).** `enabled` defaults to `true` with `when: "unfocused"`. The sound replaces a bell that `auto` already rings today, so the default adds no new noise to an unfocused terminal; it changes what that noise sounds like. `message` defaults to on, because held messages wait for the user.

### `/sounds` overlay

Rows:
- Sounds on/off;
- Pack;
- Volume (± 10);
- When;
- one row per event, with a toggle and a preview.

Keys:
- Space toggles the selected row;
- ←/→ change the pack, volume or when;
- `p` or Enter previews the selected event, which plays even while focused;
- Esc closes.

The overlay shows which player was detected, or why the bell fallback is used: "playing through afplay" or "bell only: SSH session". In `NO_COLOR` and screen-reader mode every state is a word. The binding `sounds_view` is unbound by default.

## Risks / Trade-offs

- **The Windows player starts slowly.** PowerShell takes about 300–600 ms to start, so the sound arrives late. This is acceptable for an alert, and a future change can add a small native `PlaySound` call behind `cfg(windows)` without changing the spec.
- **Linux with no sound server.** `aplay` without PulseAudio or PipeWire may fail when the device is busy. The non-zero exit rings the bell for that alert and is logged once; later alerts try the player again.
- **The first alert.** It has to render a WAV first, which takes a few ms. This happens off the UI thread in the spawned task.
- **Annoyance.** The throttle, the unfocused default and the per-event toggles limit noise. A project can never turn sounds on.

## Migration

None. `tui.notifications` keeps its meaning. A user who had `tui.notifications: "bell"` keeps the bell when `tui.sounds.pack` is `bell` or sounds are off. With sounds on, they hear the pack sound instead of the BEL.

## Open Questions

- Should the web dashboard reuse these sounds as browser audio after a click to opt in? That would be a separate change.
