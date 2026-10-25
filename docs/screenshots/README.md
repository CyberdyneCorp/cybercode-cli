# Screenshots

These are real captures of the Cyber Code TUI, not mockups. `scripts/capture_tui_screens.py` runs the local `cyber` binary in a 160×48 pseudo-terminal (Python `pty`, with [pyte](https://github.com/selectel/pyte) as the terminal emulator), writes each screen as HTML with inline colors, and turns it into a PNG with headless Chrome.

The model is a **scripted local OpenAI-compatible provider** (`scripts/screens/provider.py`): it returns fixed answers and holds chosen requests until released, so no live inference happens and nothing leaves the machine. A local stdio MCP test server stands in for an MCP connection. Everything runs in temporary HOME/CYBER_HOME directories with a small copy of this repository as the project. A second real local server supplies the direct-peer Session through its public peer routes; pairing uses a fixture-only grant. No live CyberdyneAuth, external model or personal Session data is used.

The fixture data is explicit:

- **Sessions & agents**: named local Sessions and a child are created through the owner API. The child runs a held scripted Turn. A paired loopback peer named `studio` exposes another real Session. Repeated “Review API contracts” titles are separate identities, distinguished by `peer:studio` provenance; no hub or organization membership is implied.
- **Context bar** in the sidebar: the scripted model reports 86.4K prompt tokens per reply against the demo model's 200K window, so the bar sits at 43% with the auto-compact marker at 90%.
- **Machine stats** in the sidebar (CPU, memory, disk, load) are a fixed sample (12 cores at 23%, 14.2 of 32 GB memory, 412 of 926 GB disk) that the capture hands the server through `CYBER_MACHINE_FIXTURE`, a JSON file served by `GET /system` in place of live readings, so no screenshot shows the capturing machine. Normal runs never set it.
- **Stats history** (the heatmap, streaks and totals) is seeded fixture usage for earlier days, written into the temporary database before the TUI starts. Today's usage comes from the scripted Turns.

## Regenerating

```bash
python3 -m venv /tmp/cyber-shots
/tmp/cyber-shots/bin/pip install pyte pillow
PY=/tmp/cyber-shots/bin/python just screenshots
```

`--chrome` (or `CYBER_SHOTS_CHROME`) points at another Chrome or Chromium binary; without one the script stops with a message. Playwright's installed `chrome-headless-shell` executable also works with `--chrome`. Each browser render uses a fresh temporary profile. `--binary` selects a freshly built CLI and `--out` writes elsewhere. The normal captures use 160×48 cells; the narrow drawer uses 100×30. It takes a few minutes.

The refreshed TUI captures use source `8defd812e49ea4db1181ab2215e6098b0dff06c6` and frozen CLI SHA-256 `3711f713247f32707933a76eaaa42404c51ab0caa0e117287e0f67778ae67bf4`. The capture harness's process cleanup, loopback-only HTTP and usage-fixture regressions run with:

```bash
/tmp/cyber-shots/bin/python -m unittest discover -s scripts/screens -p test_capture.py
```

## Images

| File | Screen |
|---|---|
| [`sessions-sidebar.png`](sessions-sidebar.png) | Dark 160×48 hero: the named current Session, grouped local Sessions, running child agent and paired remote Session with status/provenance |
| [`sessions-sidebar-drawer.png`](sessions-sidebar-drawer.png) | The same real fixture in a 100×30 terminal with the sidebar drawer open |
| [`welcome.png`](welcome.png) | Welcome screen: the ASCII logo, session details, tips, and the docked sidebar with Context, Sessions & agents, MCP, LSP and Machine |
| [`transcript-sidebar.png`](transcript-sidebar.png) | A finished Turn: rendered Markdown, a highlighted Python block, the `▣ Build · model · duration` marker and the sidebar context bar |
| [`thinking.png`](thinking.png), [`thinking.gif`](thinking.gif) | A Turn in progress with the Nyan comet trail, `Thinking`, the elapsed time and the interrupt hint (the GIF is 24 consecutive captures) |
| [`rename.png`](rename.png) | The `/rename` dialog, prefilled with the Session's current title |
| [`themes.png`](themes.png) | The `/theme` switcher previewing `synthwave-84` |
| [`explorer.png`](explorer.png) | The file explorer with `docs/` expanded and `docs/README.md` rendered |
| [`usage.png`](usage.png) | Settings › Usage: cost, durations, usage by model, tokens and contributing factors |
| [`stats.png`](stats.png) | Settings › Stats: activity heatmap, streaks, totals and the book comparison line |
| [`workflows-monitor.png`](workflows-monitor.png) | `/workflows` with three running runs and one queued (`#1 in queue`) |
| [`light.png`](light.png) | The transcript and default Sessions & agents sidebar in the `cyber-light` theme |

`scripts/capture_docs.py` is the older renderer for plain terminal output; its images are no longer used here.
