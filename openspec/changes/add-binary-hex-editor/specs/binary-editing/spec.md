## ADDED Requirements

### Requirement: Exact byte document access
(P1) The binary editor SHALL operate on regular files within the authorized project root using server-owned document identity, Session/Location scope, exact byte length and revision. Wire offsets/lengths SHALL be lossless decimal strings. Bounded page access SHALL preserve all byte values without Unicode conversion, implicit decompression or execution. Viewing SHALL default to 64 KiB pages with a 1 MiB maximum and SHALL NOT wait for an eager full-file hash or copy. Model read output SHALL default to 256 bytes and cap at 4 KiB plus the ordinary output budget. Larger-than-edit-limit files SHALL remain view/search only with the limit disclosed.

#### Scenario: Non-text bytes and large offset
- **WHEN** an authorized user opens NUL/invalid-UTF-8 bytes at an offset above 2^53 in a sparse regular file
- **THEN** the page preserves exact offsets/bytes and the terminal shows a bounded sanitized hex/ASCII representation without rounding or loading the whole file

### Requirement: Bounded binary navigation and interpretation
(P1) The editor SHALL support checked decimal/hex/relative goto, literal byte or UTF-8 search with explicit encoding, revision-bound continuations and up to 64 document-scoped named bookmarks. Search SHALL cap each slice at 8 MiB or 250 ms and 100 matches, preserving cross-page matches and cancellation. Live viewing/search SHALL disclose observed-generation rather than immutable-snapshot authority; reproducible revision-consistent results and patch bases SHALL require prepared immutable evidence. A read-only inspector SHALL display supported integer/float/bit interpretations with explicit endianness and unavailable values near EOF; interpretation SHALL NOT imply valid structure or alter the file.

#### Scenario: Match straddles a page boundary
- **WHEN** a literal byte sequence in a prepared immutable base starts in the last bytes of one search slice and finishes in the next
- **THEN** revision-consistent continuation returns it exactly once at its true offset

### Requirement: Atomic expected-byte draft batches
(P1) Overwrite, insert and delete SHALL stage against an exact draft/base revision as at most 128 nonoverlapping original-coordinate splices with 64 KiB total inserted/replaced bytes. All expected preimages, encodings and offset/length arithmetic SHALL validate before any part of the batch is accepted. A partial hex nibble SHALL NOT change stored bytes. Draft undo/redo SHALL remain separate from Session rewind and SHALL be bounded to 1,000 commands within the storage quota. Editing SHALL cap initial/resulting file size at 512 MiB and reserve at most 2 GiB of working storage per document; quota failure SHALL preserve existing content and dirty work.

#### Scenario: Second splice has stale bytes
- **WHEN** the first splice matches but a later splice's expected bytes differ
- **THEN** the whole batch is refused and neither the draft nor file is partially changed

### Requirement: Reviewed recoverable binary save
(P1) Saving SHALL bind exact byte-diff review, expected base/draft revision, owner/invocation authority and stable operation identity. It SHALL check read and edit ceilings, hold Cyber's workspace mutation lease, preserve a private immutable preimage and durable intent, and use proven native atomic replacement without truncating the live target in place. Metadata, displaced preimage and installed bytes SHALL be verified before success acknowledgement. Native mechanisms without the required guarantee SHALL refuse writes. External writes before replacement SHALL conflict; a detected replacement race or unprovable outcome SHALL retain recovery evidence without unconditional success or blind replay. A retry with the same identity/payload SHALL return the existing outcome; a changed payload SHALL conflict.

#### Scenario: Save acknowledgement is lost
- **WHEN** a save may have installed the reviewed bytes but its client loses the response
- **THEN** retry/lookup recovers the original operation and does not apply insertion or deletion again

#### Scenario: External editor races replacement
- **WHEN** a native save cannot prove the displaced preimage matches the reviewed base
- **THEN** it preserves displaced bytes/journal evidence and reports conflict or unknown rather than claiming a clean save or overwriting again

### Requirement: Scoped binary tool and owner authority
(P1) The capability-owned `binary` model tool SHALL expose bounded inspect/read/search/patch operations through the same byte service as owner clients. Read results SHALL require `read`; patch SHALL additionally require `edit`, Mode/sandbox/checkout authority and exact expected bytes. Read-only/Plan agents SHALL NOT patch. Permission decisions SHALL bind the actual byte ranges/revision; edit permission SHALL NOT override a denied read. Models SHALL NOT mutate the owner's unsaved draft or obtain wider filesystem access through the tool or plugin callbacks. Binary content SHALL remain untrusted data and SHALL NOT approve itself or enter model context without an explicit bounded read/share.

#### Scenario: Edit allowed but read denied
- **WHEN** a Session permits editing a path but denies reading its content
- **THEN** binary preview, before/after disclosure and patch admission remain refused without leaking bytes

### Requirement: Binary draft and restore evidence retention
(P1) Dirty drafts and unresolved saves SHALL survive disconnect/restart in owner-protected storage and SHALL require fresh scope/permission validation on resume. Clean abandoned drafts MAY be removed after 24 hours; unresolved mutation evidence SHALL NOT be expired by that rule. Every successful save SHALL retain byte-accurate preimage/receipt evidence and a separately reviewed conflict-aware Restore Last Save path, including Git/non-git files skipped by ordinary snapshot limits. Unknown effects SHALL remain unknown after local rewind; external checksums/signatures or remote effects SHALL NOT be repaired implicitly.

#### Scenario: File excluded from ordinary snapshot
- **WHEN** an authorized binary save changes a file omitted by ordinary untracked-file snapshot limits
- **THEN** its own exact preimage and receipt remain available and the UI does not claim rewind coverage without them
