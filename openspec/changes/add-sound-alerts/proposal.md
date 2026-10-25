## Why

The TUI can only get the user's attention with the terminal bell and an OSC 9 desktop notification (`crates/cyber-tui/src/osc.rs`, `tui.notifications`). The bell sounds the same for every event, many terminals mute it, and some flash instead. Someone who leaves an agent running and switches to another window can't tell by ear whether the agent finished, failed or is blocked on an approval.

There is also a gap in today's behavior. `tui` → Attention notifications (P0) says the user is notified when a Session **needs input**, but `attention()` is only called on `session.idle`, Session notifications and workflow-run ends. A permission or question request arrives with no notification, which is the case that matters most for an unattended agent.

## What Changes

- **Sound alerts** (`tui`, added "Sound alerts"). The TUI plays a short, distinct sound for each alert event:
  - `input`: a permission or question request is waiting;
  - `done`: the Session finished its work and is idle;
  - `error`: a Turn failed;
  - `workflow`: a watched workflow run completed;
  - `workflow_failed`: a watched workflow run ended any other way (failed, stopped, over budget, paused by a restart);
  - `message`: a cross-session message arrived or is held.
  
  Sounds play only while the terminal is unfocused by default (`tui.sounds.when: "unfocused"`). Playback never blocks a frame or a key, and alerts are throttled so a burst produces one sound.
- **Sound packs** (`tui`, added "Sound packs"). Four built-in packs:
  - `cyber` (default): short FM chimes;
  - `soft`: marimba-like sine tones;
  - `retro`: 8-bit square-wave arpeggios;
  - `bell`: no audio, the terminal bell only.
  
  The sounds are synthesized in code, so there are no audio assets, no licensing questions and no growth in binary size. They are rendered once to WAV files in the cache directory. A user can replace any event's sound with their own file, but only from the global `tui.jsonc`.
- **Playback.** Sounds play through the platform's own player: `afplay` on macOS; `pw-play`, `paplay` or `aplay` on Linux; `System.Media.SoundPlayer` through PowerShell on Windows. This means no native audio dependency, which keeps the musl and cross-compiled release targets building.
  - With no player, or in an SSH session (where the sound would play on the remote machine), the alert falls back to the terminal bell, which the user's own terminal plays.
- **`/sounds`** (`tui`, added "Sound settings command"). An overlay, also in the command palette, to:
  - turn sounds on or off;
  - pick a pack;
  - set the volume;
  - turn individual events on or off;
  - preview each sound.
  
  The choices are saved to `kv.json` in the same way as `/theme`.
- **Needs-input attention** (`tui`, added "Needs-input attention"). A new permission or question request fires the attention notification (`tui.notifications`) and the `input` sound, once per request, including after a reconnect. This closes the gap in Attention notifications.
- **Configuration.** New `tui.sounds` keys in `tui.jsonc`:
  - `enabled`, `pack`, `volume`, `when` and `events`;
  - per-event `false` turns an event off, and a per-event path sets a custom file;
  - `play_over_ssh`.
  
  A project `tui.jsonc` may only turn sounds **off** or lower the volume. A project's files may be untrusted, so it cannot point at files or enable playback.

## Non-goals (deferred)

- **Sounds in the web dashboard.** Browsers need a user gesture before they will play audio, so the dashboard needs its own design. A separate change can reuse the same event names.
- **The native OS notifier** (macOS Notification Center, libnotify, Windows toast) that Attention notifications lists. It is a separate notification channel, not a sound.
- **Device push** (P3, `remote-control`).
- **Text-to-speech** and spoken alerts.
- **Sounds from `cyber exec` or the server.** The server never plays audio, and only interactive TUI clients do.
- **Per-project sound themes.** A project layer can only turn sounds down.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `tui`: added "Sound alerts", "Sound packs", "Sound settings command" and "Needs-input attention". No existing requirement is modified, so this change cannot conflict with open changes. The added "Needs-input attention" spells out the needs-input half of Attention notifications without editing that requirement.

## Impact

- **Code:** `crates/cyber-tui`:
  - new `sound/` (`synth.rs`, `player.rs`, `mod.rs`);
  - `app.rs` (alert dispatch on events and new requests);
  - `osc.rs` (bell fallback shared with `attention`);
  - `prefs.rs` (`tui.sounds` and its persistence);
  - `palette.rs`, `keys.rs` and `view/sounds.rs` (`/sounds`).
  
  `crates/cyber-core/src/config/validate.rs` gets the `sounds` TUI key.
- **No server, API, SDK or CLI changes.**
- **New dependency:** none. WAV encoding is about 30 lines of code, and players are found on `PATH`.
- **Docs:** the TUI section of `docs/local-core-guide.md`, `docs/features.md`, and the ROADMAP entry for sound alerts.
