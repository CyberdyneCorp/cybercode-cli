# Hex workspace

The visual identity is Cyber's existing semantic theme: quiet pane borders, a strong focused-pane accent, byte groups with breathing room, and small explicit state/key chips. No new font pack is required. Selection, edits and matches have both color and a structural marker; NO_COLOR remains fully usable.

```text
 CYBER / FILES / firmware.bin                    HEX   VIEW   65,536 bytes
 ┌─ files ───────────┬─ offset ── bytes ─────────────────────── ASCII ─────┐
 │ firmware.bin  M  │ 00000000   7F 45 4C 46  02 01 01 00     .ELF....  │
 │ fixtures/        │ 00000008   00 00 00 00  00 00 00 00     ........  │
 │ src/             │ 00000010   02 00 3E 00  01 00 00 00     ..>.....  │
 │                  │           └ shared byte selection ┘               │
 │                  ├─ INSPECTOR ───────────────────────────────────────┤
 │                  │ offset 0x10  uint16 2  endian LE  bits 00000010    │
 └──────────────────┴───────────────────────────────────────────────────┘
  offset 0x10 / 0x10000   selection 2 bytes   no staged changes
  [e edit] [g offset] [/ find] [b bookmark] [Tab pane] [? commands]
```

The diagram uses an eight-byte row for compact presentation. The actual row width adapts to available columns; offsets, bytes and ASCII always describe the same range.

## Interaction

| Action | Proposed behavior |
|---|---|
| Open | Explorer Open in Hex / `x`, `/hex path` or `cyber hex path`; start in VIEW |
| Navigate | Arrow/Page/Home/End keys, mouse selection/scroll and exact goto offset; no implicit edits |
| Inspect | Shared cursor in hex/ASCII; toggle inspector and LE/BE; unavailable values are labelled |
| Edit | `e` enters an explicitly labelled draft; hex nibble input or literal text entry with explicit encoding |
| Resize | Explicit Insert/Delete/Paste commands show the byte-count change before staging |
| Undo/redo | Editor-scoped undo/redo commands affect only unsaved draft history; keybindings use existing context precedence |
| Save | Ctrl+S requests review, then an explicit save action; never write on blur or navigation |
| Search | `/` opens literal hex/text search; next/previous reuse a revision-scoped result set |
| Bookmark | `b` adds a bounded name at the exact offset; palette lists bookmarks for this document |
| Share with agent | Explicit Copy Offset/Copy Hex or attach a bounded selected range; never send the full binary implicitly |
| Leave | A dirty draft offers Keep draft, Discard or Review/save; Esc alone does not discard changes |

```text
 firmware.bin                      HEX   REVIEW   2 ranges   net +1 byte
 original offset     before                    after
 0x00000010          02 00                     03 00           overwrite
 0x00000020          7F                        7E 00           replace/resize

 base  SHA256 ...      final size 65,537 bytes      permissions preserved
 [Save reviewed bytes] [Back to edit] [Discard draft]
```

Show the actual base/proposed digest and size from the server; the shortened values here are layout placeholders. Range totals and final length must be internally consistent. Conflicts become a dedicated state with expected/current fingerprints and a bounded byte comparison, with Reload/Keep draft/Review recovery choices. There is no automatic force-overwrite button.

## Responsive and accessible states

- Wide: file tree, hex/ASCII grid and optional inspector visible together. Medium: inspector drawer. Narrow: hex/ASCII lanes toggle with the same cursor; reduce bytes per row without losing absolute offset or dirty state.
- Use theme tokens for addresses, normal bytes, zero bytes, selections, inserted/deleted/changed ranges and matches. Brightness is subordinate to readable contrast; color is never the only signal.
- Announce cursor offset, selected byte count, raw hex value, interpreted value, dirty state and operation result through the existing accessibility conventions. The command palette exposes every action; key chips reflect rebinding rather than hardcoded labels.
- Preserve global interrupt, pending permission handling and native emergency Stop priority even while typing a hex nibble, searching or reviewing a save. Clipboard, filenames, ASCII previews and error text are sanitized.
- Preparation, search and save show truthful progress and cancellation state. Never animate a fake successful save while the server has an unknown outcome.
