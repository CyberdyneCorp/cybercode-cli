# Reviewed rewind

`cyber sessions rewind` previews a rewind before applying it. It does not start
inference or interrupt a busy Session automatically. An existing staged revert
must be explicitly committed or cleared before starting this reviewed flow.

```sh
cyber sessions rewind ses_example --to msg_example --target both
# Review the proposed patches, exclusions and effect disclosure, then:
cyber sessions rewind ses_example --to msg_example --target both --confirm <fingerprint>
```

Use `--format json` for the typed preview/result. `--to` takes an exact active
user-message ID; Session selection accepts the usual ID, title or unique prefix.
Every target requires explicit confirmation. `code` restores eligible files and
keeps the conversation; `conversation` moves the active path before that message;
`both` does both. Later conversation messages remain reachable through
`cyber sessions tree`, checkout and `export --all-branches`.

The fingerprint binds the Session, exact Location, sequence, boundary, active tip,
snapshot identities, proposed patches, current exclusions and retained tool status.
A changed context requires a fresh review. Code planning uses the existing
three-way merge rules; unrelated owner files stay intact and are not shown as
proposed deletions. Conflicts are reported before application. Planning uses a
separate shadow Git index and never installs proposed files into the workspace.

Confirmation obtains native Location ownership, rechecks the current tree under
the workspace mutation lock and refreshes admission before file changes. A failed
native acknowledgement or event commit after a possible effect retains the native
activity fence and backup evidence; it is not a successful rewind. Do not repeat a
mutation to infer its outcome. Inspect the Session and retained recovery evidence.

Code rewind currently needs an available Git snapshot with an implementing
reviewed-restore host. Non-Git Locations and disabled snapshots support
conversation-only rewind; their code rewind fails visibly. Current snapshot
exclusions are listed; complete historical exclusion reporting remains acceptance
work. Legacy stages keep their original owner-route behavior. A reviewed file stage
cannot be replaced or cleared through legacy stage/clear routes, or discarded by
branch checkout; use a reviewed clear or explicitly commit it.

Owner routes, under the normal authenticated server boundary:

- `POST /api/v1/sessions/{id}/rewind/preview` with `message_id`, `target`,
  `expected_directory` and optional `action` (`commit`, the default; `stage`;
  `clear`) returns the preview and `confirmation` fingerprint.
- `POST /api/v1/sessions/{id}/rewind/confirm` takes those selection fields plus the
  reviewed fingerprint. No client-supplied snapshot identity is trusted.

The response is limited to 4 MiB. This bounds the response, not peak Git planning
memory. The TypeScript SDK exposes both generated operations. Fingerprints are
review identities, not authentication credentials. Rewind does not undo remote
calls, sent messages or unknown tool/native outcomes and does not guarantee atomic
replacement against arbitrary external writers.

## Terminal menu and undo/redo

Open `/rewind`, or press Esc twice within 500 ms while idle. Select an active
user-message boundary, choose `1` code, `2` conversation or `3` both, and press
Enter to preview. Enter again explicitly commits the reviewed rewind. This menu
preserves the current composer. Scroll the review with arrows or PgUp/PgDn.
`/rewind`, `/undo` and `/redo` are built-ins in the shared command catalog,
available through slash completion and `GET /commands`.

![Rewind boundary menu](screenshots/rewind-menu.png)

The review shows the actual proposed patches, current exclusions, retained tool
statuses and external-effect warning before confirmation.

![Reviewed code and conversation rewind](screenshots/rewind-review.png)

`/undo` reviews a staged rewind to the latest user boundary. Confirm to restore
eligible files and copy the boundary's plain prompt text into the composer.
Repeated `/undo` moves backward; `/redo` reviews movement forward. The final
`/redo` reviews clearing the stage and restoring the original baseline. Each
transition needs a fresh review and explicit confirmation. The staged boundary
commits on the next prompt, shell command or compaction; the header shows
`UNDO STAGED` until then. The target stays fixed during a staged chain.

Opening rewind while busy does not interrupt work. Press `I` to request
interruption, then `R` to refresh if the server has not yet acknowledged idle.
Normal busy Esc still interrupts; permission/question prompts and the native
emergency stop retain priority. No rewind starts inference.

Composer replacement requires an acknowledged operation for the exact Session,
Location and panel generation, with no intervening composer edit or attachment.
Newer text stays intact. The original plain-text draft is retained in memory for
up to 32 Session/Location pairs; `P` in the menu copies a retained original. Clear
restores it only while the composer still matches the last restored prompt.
Undo/redo refuses unsent attachment chips; historical media attachments are not
recreated. After a TUI restart, retained server stages remain available but the
original in-memory draft is unavailable, so clear preserves the current composer.

Reviewed file stages durably retain the original baseline, selected agent snapshot
and union of paths actually changed (maximum 4096). Clear only restores that path
set through the existing three-way planner; unrelated owner files are not adopted
into redo authority. Old stages without this manifest use their original clear
route. After an ambiguous response, the panel drops its preview: `R` reads current
status, without automatically replaying the mutation or replacing the composer.

The screenshots are real terminal captures against a local scripted provider.
The capture fixture writes the sample edit during a held Turn to exercise actual
Git snapshots and preview rendering; it does not confirm the rewind.

This implements the CLI and TUI rewind surfaces of
[#93](https://github.com/CyberdyneCorp/cybercode-cli/issues/93). Broader native
crash/restart/interrupt acceptance, complete historical exclusion reporting, the
private non-Git backend and recoverable restore journal remain unfinished. Path
rules/context, root structured output and JSONL input are also outstanding. The
retry-count specification reconciliation does not implement root structured output.
