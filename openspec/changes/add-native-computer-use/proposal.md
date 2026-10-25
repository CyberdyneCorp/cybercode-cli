# Native computer use for local agents

## Why

Browser verification cannot inspect or operate native applications. The user requested screenshots, mouse movement/clicks and editing through keyboard input, similar to an interactive computer-use assistant. They selected **macOS first**, with explicit unsupported-platform messages on Windows/Linux and a portable public API.

## What Changes

- Add an experimental, global opt-in `computer_use` capability, initially on macOS 14+ using native screen capture and accessibility/event APIs.
- Provide screenshot, mouse move/click/drag/scroll, text entry, key chord, bounded wait and status actions to model tools through the ordinary permission/runtime pipeline.
- Require an explicit owner-armed, Session/target-bound short-lived control grant, current OS permissions and fresh screenshot coordinates. Add an immediate stop path independent of the model.
- Add `cyber computer` setup/status/allow/stop operations, typed API/SDK and terminal/dashboard indications and review cards. Retain bounded private action/evidence records with truthful unknown outcomes after interruption.
- Update the desktop-wrapper spec's blanket exclusion: native computer use is a separate capability and does not depend on shipping a Tauri application.

## Scope

The first release supports macOS 14+ only; other platforms return a typed unsupported-platform result. Models need image capability for visual interaction. Editing means native UI text/key operations under an explicit target grant; code file editing continues through existing workspace tools. Neither Cyber Account nor the live Auth service is required for local use.

No background screen surveillance, audio/video recording, arbitrary AppleScript/shell execution as a computer action, unattended control grant, credential extraction, keylogging, clipboard scraping, remote hub control authority or guaranteed correctness from visual observation. A project file or model message cannot enable the feature or arm itself. The user approved the linked designs and requested implementation on 2026-10-24.

The owner-only control boundary also tightens local transport security: Unix owner APIs require the existing password in addition to same-UID credentials, protected owner state cannot be exposed through model tools/environments, and custom/legacy socket paths outside private state are refused before credentials are sent. See the server-api delta and design rationale.
