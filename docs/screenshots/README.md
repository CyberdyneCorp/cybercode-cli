# Screenshots

These are real captures of the Cyber Code TUI, not mockups or generated product screens. They were taken on **2026-10-16** on macOS arm64 with the local debug binary, during the acceptance runs for the TUI overhaul (`improve-tui-experience`) and parallel workflow runs (`add-parallel-workflow-runs`).

The model is a **scripted local OpenAI-compatible provider**: it returns fixed answers, so no live inference happened. Session names, token counts and costs come from those scripted turns.

## How they were made

1. A real local `cyber` server ran in a temporary home with a demo project and the scripted provider configured.
2. `./target/debug/cyber` ran in a 160×48 pseudo-terminal (Python `pty`), with [pyte](https://github.com/selectel/pyte) as the terminal emulator.
3. Each pyte screen was written to HTML with inline colors and saved as a PNG. The HTML title band was then cropped off with Pillow.

## Images

| File | Screen |
|---|---|
| [`welcome.png`](welcome.png) | Welcome screen: ASCII logo, agent/model/mode/path/MCP/LSP details, tips, and the docked sidebar |
| [`transcript-sidebar.png`](transcript-sidebar.png) | A turn with rendered Markdown (bold, lists, highlighted Python block), the `▣ Build · model · duration` marker, and the sidebar with Context, MCP and LSP |
| [`stats.png`](stats.png) | Settings › Stats › Overview: activity heatmap, period and project filters, sessions, streaks, tokens and cost |
| [`workflows-monitor.png`](workflows-monitor.png) | `/workflows` monitor with three running runs and one queued (`#1 in queue`), agent counts `done/running/queued/failed`, and the `⚙ 4 workflows · 1 queued` footer |

## Older captures

`tui.png` (offline startup at 112×30) and `cli.png` (the commands section of `cyber --help`) were captured on 2026-10-10 by `scripts/capture_docs.py`, which renders terminal output with Monaco:

```bash
python3 -m venv /tmp/cyber-docs-render-env
/tmp/cyber-docs-render-env/bin/pip install pillow pyte
/tmp/cyber-docs-render-env/bin/python scripts/capture_docs.py
```

That script creates and removes its own temporary home and demo repository and runs a private embedded server. On other systems, pass `--font /path/to/monospace.ttf`; use `--binary` to select another built CLI. It does not produce the four TUI captures above.
