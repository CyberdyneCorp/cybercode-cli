## 1. Synthesis and cache

- [ ] 1.1 Add `crates/cyber-tui/src/sound/synth.rs`, which covers `Note`, the waveforms (sine, FM, square, triangle), the ADSR envelope, mixing, normalising to −3 dBFS, volume scaling and the WAV writer.
  - Add the pack tables for `cyber`, `soft` and `retro` from the design, and `SYNTH_VERSION`.
  - Unit tests:
    - a pinned SHA-256 for every pack and event at volume 60;
    - peak ≤ −1 dBFS;
    - the first and last 2 ms are below −40 dBFS;
    - each sound lasts ≤ 0.9 s;
    - the six sounds of each pack are pairwise distinct;
    - volume 0 renders silence.
- [ ] 1.2 Add the cache in `sound/mod.rs`: file names `<pack>-<event>-v<SYNTH_VERSION>-<volume>.wav` under the cache directory, an atomic write (temp file then rename), and best-effort removal of older versions.
  - Unit tests:
    - a second request reuses the file;
    - another volume makes a new file;
    - a stale version is removed.

## 2. Player and fallback

- [ ] 2.1 Add `sound/player.rs`:
  - detection by OS and `PATH` order;
  - the argv for each player, with no shell;
  - on Windows, the path in `CYBER_SOUND_FILE`;
  - null stdio, and a separate process group on Unix;
  - a 5 s kill, kill on a newer sound, kill on exit.
  
  Add a `Player` trait so tests can record calls instead of spawning.
  - Unit tests:
    - detection order with a fake `PATH`;
    - the Windows command string never contains the path;
    - `SSH_CONNECTION` or `SSH_TTY` without `play_over_ssh` chooses the bell;
    - a spawn error chooses the bell, logged once.
- [ ] 2.2 Production-path test on Unix: put a stub `afplay`/`pw-play` script on `PATH` that records its argv to a file, fire an alert through a real `App` and runner, and check that the stub got the cached WAV path and exited.

## 3. Alert dispatch

- [ ] 3.1 Add `Alert` and `App::alert(event, text)`, and route every alert source through it:
  - the existing `attention` calls in `app.rs` (`session.idle`), `side_chat.rs` and `workflow_watch.rs`, split into `workflow` and `workflow_failed` by the run status;
  - new calls for `session.error` / a failed Turn, and for an incoming delivered or held message (`messaging.rs`).
  
  Leave out the BEL of `auto` when a sound played.
- [ ] 3.2 Add the throttle: a 1.5 s window, priority replacement of a waiting alert, and the drop counter. Unit tests cover "Burst coalesces", the higher-priority replacement, and that a preview bypasses the throttle.
- [ ] 3.3 Needs-input: alert when a snapshot or event shows a request ID not seen before, using a bounded set of 512 seen IDs. Regression test for "Permission request notifies once": the request is shown again after a reconnect and alerts only once. Also test that a question request alerts and that a focused terminal stays quiet.
- [ ] 3.4 Screen tests (`screens_indicators.rs` style, `attention_log` plus a recorded `Player`) for each scenario of "Sound alerts":
  - approval while away;
  - focused terminal stays quiet;
  - SSH falls back to the bell;
  - no player.

## 4. Configuration

- [ ] 4.1 Add the `sounds` TUI key (`cyber-core` `config/validate.rs` `TUI_KEYS`), and parse `tui.sounds` in `prefs.rs`:
  - `enabled`, `pack`, `volume`, `when`, `play_over_ssh`, `events` (bool, pack name or path);
  - the layer order defaults < global < `kv.json` < project restrictions.
  
  Paths are validated (global only, regular file, ≤ 5 MiB, allowed extension), and invalid values give warnings.
  - Unit tests:
    - "Project cannot pick a file";
    - "Custom file from global config";
    - a project may lower the volume but not raise it;
    - unknown keys warn.

## 5. `/sounds` overlay

- [ ] 5.1 Add `Overlay::Sounds`, the `/sounds` command, a palette entry, the `sounds_view` binding (unbound), the help row, and `view/sounds.rs` (rows, player status line, word labels). Add `/sounds` to the built-in command catalog (`cyber-core/src/commands.rs`).
- [ ] 5.2 Add key handling and persistence to `kv.json`, and pre-render the current pack when the overlay opens.
  - Screen tests:
    - dark, light and no-color snapshots;
    - "Pick a pack and preview", including surviving a restart;
    - "Fallback shown";
    - toggling an event off silences it.

## 6. Docs and checks

- [ ] 6.1 Document sound alerts, the packs, `/sounds`, `tui.sounds`, SSH behaviour and the terminal focus-reporting caveat in the TUI section of `docs/local-core-guide.md`. Add a row to `docs/features.md` and mark the ROADMAP entry implemented.
- [ ] 6.2 Run `cargo fmt`, workspace clippy with `-D warnings`, `cargo nextest run --workspace --profile ci`, `openspec validate --all --strict` and `scripts/spec_lint.py` (no new warnings). Keep per-function cognitive complexity ≤ 15.
- [ ] 6.3 Check on the real binary on macOS and Linux (PipeWire): each pack's six sounds through `/sounds`, and an unfocused approval alert.
