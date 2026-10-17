# Screenshots

These are real captures of the Cyber Code TUI, not mockups. `scripts/capture_tui_screens.py` runs the local `cyber` binary in a 160×48 pseudo-terminal (Python `pty`, with [pyte](https://github.com/selectel/pyte) as the terminal emulator), writes each screen as HTML with inline colors, and turns it into a PNG with headless Chrome.

The model is a **scripted local OpenAI-compatible provider** (`scripts/screens/provider.py`): it returns fixed answers and holds chosen requests until released, so no live inference happens and nothing leaves the machine. A local stdio MCP test server stands in for an MCP connection. Everything runs in a temporary HOME/CYBER_HOME with a small copy of this repository as the project.

Two things are not scripted:

- **Machine stats** in the sidebar (CPU, memory, disk, load) are live readings of the machine that took the captures.
- **Stats history** (the heatmap, streaks and totals) is seeded fixture usage for earlier days, written into the temporary database before the TUI starts. Today's usage comes from the scripted Turns.

## Regenerating

```bash
python3 -m venv /tmp/cyber-shots
/tmp/cyber-shots/bin/pip install pyte pillow
PY=/tmp/cyber-shots/bin/python just screenshots
```

`--chrome` (or `CYBER_SHOTS_CHROME`) points at another Chrome or Chromium binary; without one the script stops with a message. `--out` writes elsewhere. It takes about two minutes.

## Images

| File | Screen |
|---|---|
| [`welcome.png`](welcome.png) | Welcome screen: the ASCII logo, session details, tips, and the docked sidebar with Context, MCP, LSP and Machine |
| [`transcript-sidebar.png`](transcript-sidebar.png) | A finished Turn: rendered Markdown, a highlighted Python block and the `▣ Build · model · duration` marker |
| [`thinking.png`](thinking.png), [`thinking.gif`](thinking.gif) | A Turn in progress with the Nyan comet trail, `Thinking`, the elapsed time and the interrupt hint (the GIF is 24 consecutive captures) |
| [`rename.png`](rename.png) | The `/rename` dialog, prefilled with the Session's current title |
| [`themes.png`](themes.png) | The `/theme` switcher previewing `synthwave-84` |
| [`explorer.png`](explorer.png) | The file explorer with `docs/` expanded and `docs/README.md` rendered |
| [`usage.png`](usage.png) | Settings › Usage: cost, durations, usage by model, tokens and contributing factors |
| [`stats.png`](stats.png) | Settings › Stats: activity heatmap, streaks, totals and the book comparison line |
| [`workflows-monitor.png`](workflows-monitor.png) | `/workflows` with three running runs and one queued (`#1 in queue`) |
| [`light.png`](light.png) | The transcript and sidebar in the `cyber-light` theme |

`scripts/capture_docs.py` is the older renderer for plain terminal output; its images are no longer used here.
