//! Bounded piece-table drafts. The source must be a prepared immutable base, never
//! a live path. This module changes no files and grants no read/write authority.
use super::{Error, MAX_EDIT_SIZE, MAX_PAGE, Offset, Page, decode_hex};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const MAX_SPLICES: usize = 128;
pub const MAX_PATCH_BYTES: usize = 64 * 1024;
pub const MAX_HISTORY: usize = 1000;

/// Implemented by the retained private base. Reads must return the exact range.
/// The caller establishes privacy, identity, capacity and durable ownership first.
pub trait ImmutableBase {
    fn length(&self) -> u64;
    fn sha256(&self) -> &str;
    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>, Error>;
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Splice {
    pub offset: Offset,
    pub delete_length: Offset,
    pub expected_hex: String,
    pub insert_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchBatch {
    pub revision: String,
    pub splices: Vec<Splice>,
}

#[derive(Clone)]
struct Edit {
    at: u64,
    expected: Arc<[u8]>,
    inserted: Arc<[u8]>,
}
#[derive(Clone)]
struct Command {
    forward: Vec<Edit>,
    inverse: Vec<Edit>,
}
#[derive(Clone)]
enum Bytes {
    Base,
    Added(Arc<[u8]>),
}
#[derive(Clone)]
struct Piece {
    bytes: Bytes,
    offset: u64,
    length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChangeRange {
    pub original_offset: Offset,
    pub original_length: Offset,
    pub draft_offset: Offset,
    pub draft_length: Offset,
    pub before_sha256: String,
    pub after_sha256: String,
    pub before_hex: String,
    pub after_hex: String,
    pub preview_truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Review {
    pub revision: String,
    pub base_sha256: String,
    pub proposed_sha256: String,
    pub base_length: Offset,
    pub proposed_length: Offset,
    pub original_bytes: Offset,
    pub proposed_bytes: Offset,
    pub range_count: usize,
    pub ranges: Vec<ChangeRange>,
    pub next_range: Option<usize>,
    pub fingerprint: String,
}

/// Durable encoding contains commands, not raw native paths or executable inputs.
/// Its enclosing store must authenticate/bind the base, owner, identity and journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftState {
    version: u32,
    nonce: String,
    epoch: u64,
    base_sha256: String,
    base_length: Offset,
    commands: Vec<Vec<Splice>>,
    cursor: usize,
}

#[derive(Clone)]
pub struct Draft<B> {
    base: B,
    nonce: String,
    epoch: u64,
    pieces: Vec<Piece>,
    history: Vec<Command>,
    cursor: usize,
    length: u64,
}

impl<B: ImmutableBase> Draft<B> {
    pub fn new(base: B) -> Result<Self, Error> {
        if base.length() > MAX_EDIT_SIZE {
            return Err(Error::Limit);
        }
        if base.sha256().len() != 64
            || !base
                .sha256()
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(Error::Invalid);
        }
        let length = base.length();
        let pieces = if length == 0 {
            vec![]
        } else {
            vec![Piece {
                bytes: Bytes::Base,
                offset: 0,
                length,
            }]
        };
        Ok(Self {
            base,
            nonce: crate::ids::new_id("draft"),
            epoch: 0,
            pieces,
            history: vec![],
            cursor: 0,
            length,
        })
    }

    pub fn revision(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"cyber-binary-draft-v1\0");
        hash.update(self.base.sha256());
        hash.update(self.nonce.as_bytes());
        hash.update(self.epoch.to_le_bytes());
        format!("{:x}", hash.finalize())
    }
    pub fn length(&self) -> Offset {
        Offset(self.length)
    }
    pub fn base(&self) -> &B {
        &self.base
    }
    pub fn can_undo(&self) -> bool {
        self.cursor != 0
    }
    pub fn can_redo(&self) -> bool {
        self.cursor < self.history.len()
    }

    fn current(&self, revision: &str) -> Result<(), Error> {
        if revision != self.revision() {
            return Err(Error::Changed);
        }
        Ok(())
    }

    fn mutation(&self, revision: &str) -> Result<(), Error> {
        self.current(revision)?;
        if self.epoch == u64::MAX {
            return Err(Error::Limit);
        }
        Ok(())
    }

    pub fn page(&self, revision: &str, at: Offset, limit: usize) -> Result<Page, Error> {
        self.current(revision)?;
        if limit == 0 || limit > MAX_PAGE || at.0 > self.length {
            return Err(Error::Limit);
        }
        let length = limit.min((self.length - at.0) as usize);
        let bytes = self.read(at.0, length)?;
        Ok(Page {
            offset: at,
            eof: at.0 + length as u64 == self.length,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
        })
    }

    fn piece_bytes(&self, piece: &Piece, offset: u64, length: usize) -> Result<Vec<u8>, Error> {
        match &piece.bytes {
            Bytes::Base => {
                let bytes = self.base.read(piece.offset + offset, length)?;
                if bytes.len() != length {
                    return Err(Error::Io);
                }
                Ok(bytes)
            }
            Bytes::Added(bytes) => bytes
                .get((piece.offset + offset) as usize..(piece.offset + offset) as usize + length)
                .map(<[u8]>::to_vec)
                .ok_or(Error::Invalid),
        }
    }

    /// A bounded read; base-sized allocation is never required by editing.
    pub fn read(&self, at: u64, length: usize) -> Result<Vec<u8>, Error> {
        if length > MAX_PAGE
            || at
                .checked_add(length as u64)
                .is_none_or(|end| end > self.length)
        {
            return Err(Error::Limit);
        }
        let mut output = Vec::with_capacity(length);
        let mut position = 0;
        for piece in &self.pieces {
            let end = position + piece.length;
            if end > at && output.len() < length {
                let skip = at.saturating_sub(position);
                let take = (length - output.len()).min((piece.length - skip) as usize);
                output.extend_from_slice(&self.piece_bytes(piece, skip, take)?);
            }
            position = end;
            if output.len() == length {
                break;
            }
        }
        if output.len() != length {
            return Err(Error::Io);
        }
        Ok(output)
    }

    pub fn patch(&mut self, batch: &PatchBatch) -> Result<String, Error> {
        self.mutation(&batch.revision)?;
        if self.cursor >= MAX_HISTORY {
            return Err(Error::Limit);
        }
        let edits = decode(&batch.splices, self.length)?;
        for edit in &edits {
            if self.read(edit.at, edit.expected.len())?.as_slice() != edit.expected.as_ref() {
                return Err(Error::Changed);
            }
        }
        let (pieces, length) = self.splice(&edits)?;
        let inverse = inverse(&edits)?;
        // All validation and allocation precedes this one in-memory commit.
        self.pieces = pieces;
        self.length = length;
        self.history.truncate(self.cursor);
        self.history.push(Command {
            forward: edits,
            inverse,
        });
        self.cursor += 1;
        self.epoch += 1;
        Ok(self.revision())
    }

    pub fn undo(&mut self, revision: &str) -> Result<String, Error> {
        self.mutation(revision)?;
        let command = self
            .history
            .get(self.cursor.checked_sub(1).ok_or(Error::Invalid)?)
            .ok_or(Error::Invalid)?;
        let (pieces, length) = self.splice(&command.inverse)?;
        self.pieces = pieces;
        self.length = length;
        self.cursor -= 1;
        self.epoch += 1;
        Ok(self.revision())
    }
    pub fn redo(&mut self, revision: &str) -> Result<String, Error> {
        self.mutation(revision)?;
        let command = self.history.get(self.cursor).ok_or(Error::Invalid)?;
        let (pieces, length) = self.splice(&command.forward)?;
        self.pieces = pieces;
        self.length = length;
        self.cursor += 1;
        self.epoch += 1;
        Ok(self.revision())
    }

    fn slice(&self, from: u64, to: u64, output: &mut Vec<Piece>) {
        let mut position = 0;
        for piece in &self.pieces {
            let end = position + piece.length;
            let start = from.max(position);
            let finish = to.min(end);
            if finish > start {
                output.push(Piece {
                    bytes: piece.bytes.clone(),
                    offset: piece.offset + start - position,
                    length: finish - start,
                });
            }
            position = end;
            if position >= to {
                break;
            }
        }
    }

    fn splice(&self, edits: &[Edit]) -> Result<(Vec<Piece>, u64), Error> {
        let mut pieces = Vec::new();
        let mut last = 0;
        for edit in edits {
            self.slice(last, edit.at, &mut pieces);
            if !edit.inserted.is_empty() {
                pieces.push(Piece {
                    bytes: Bytes::Added(edit.inserted.clone()),
                    offset: 0,
                    length: edit.inserted.len() as u64,
                });
            }
            last = edit.at + edit.expected.len() as u64;
        }
        self.slice(last, self.length, &mut pieces);
        let length = pieces.iter().map(|piece| piece.length).sum();
        if length > MAX_EDIT_SIZE {
            return Err(Error::Limit);
        }
        Ok((pieces, length))
    }

    /// Bounded streaming for review and replacement preparation. This does not
    /// install a file; callers still need native admission and a durable save journal.
    pub fn write_to(
        &self,
        writer: &mut dyn std::io::Write,
        cancelled: impl Fn() -> bool,
    ) -> Result<String, Error> {
        let mut hash = Sha256::new();
        let mut at = 0;
        while at < self.length {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let bytes = self.read(
                at,
                (self.length - at).min(super::DEFAULT_PAGE as u64) as usize,
            )?;
            writer.write_all(&bytes).map_err(|_| Error::Io)?;
            hash.update(&bytes);
            at += bytes.len() as u64;
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    fn range_hash(
        &self,
        original: bool,
        at: u64,
        length: u64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<String, Error> {
        let mut hash = Sha256::new();
        let mut done = 0;
        while done < length {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let size = (length - done).min(super::DEFAULT_PAGE as u64) as usize;
            let bytes = if original {
                self.base.read(at + done, size)?
            } else {
                self.read(at + done, size)?
            };
            if bytes.len() != size {
                return Err(Error::Io);
            }
            hash.update(&bytes);
            done += size as u64;
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    fn ranges(&self) -> Vec<(u64, u64, u64, u64)> {
        let (mut original, mut after, mut start) = (0, 0, 0);
        let mut ranges = Vec::new();
        for piece in &self.pieces {
            if matches!(piece.bytes, Bytes::Base) {
                if piece.offset > original || after > start {
                    ranges.push((original, piece.offset - original, start, after - start));
                }
                original = piece.offset + piece.length;
                after += piece.length;
                start = after;
            } else {
                after += piece.length;
            }
        }
        if original < self.base.length() || start < self.length {
            ranges.push((
                original,
                self.base.length() - original,
                start,
                self.length - start,
            ));
        }
        ranges
    }

    /// Original coordinates and fingerprints cover all ranges; previews are bounded
    /// and pageable. Revisiting a page does not alter the review identity.
    pub fn review(
        &self,
        revision: &str,
        start: usize,
        limit: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<Review, Error> {
        self.current(revision)?;
        if limit == 0 || limit > MAX_SPLICES {
            return Err(Error::Limit);
        }
        if self.range_hash(true, 0, self.base.length(), &cancelled)? != self.base.sha256() {
            return Err(Error::Changed);
        }
        let proposed_sha256 = self.write_to(&mut std::io::sink(), &cancelled)?;
        let mut proof = Sha256::new();
        proof.update(b"cyber-binary-review-v1\0");
        proof.update(revision);
        proof.update(self.base.sha256());
        proof.update(&proposed_sha256);
        let (mut original_bytes, mut proposed_bytes, mut count) = (0, 0, 0);
        let mut ranges = Vec::new();
        for (before, removed, after, inserted) in self.ranges() {
            let before_sha256 = self.range_hash(true, before, removed, &cancelled)?;
            let after_sha256 = self.range_hash(false, after, inserted, &cancelled)?;
            if removed == inserted && before_sha256 == after_sha256 {
                continue;
            }
            for number in [before, removed, after, inserted] {
                proof.update(number.to_le_bytes());
            }
            proof.update(&before_sha256);
            proof.update(&after_sha256);
            if count >= start && ranges.len() < limit {
                let before_hex = encode_hex(&self.base.read(before, removed.min(256) as usize)?);
                let after_hex = encode_hex(&self.read(after, inserted.min(256) as usize)?);
                ranges.push(ChangeRange {
                    original_offset: Offset(before),
                    original_length: Offset(removed),
                    draft_offset: Offset(after),
                    draft_length: Offset(inserted),
                    before_sha256,
                    after_sha256,
                    before_hex,
                    after_hex,
                    preview_truncated: removed > 256 || inserted > 256,
                });
            }
            original_bytes += removed;
            proposed_bytes += inserted;
            count += 1;
        }
        if start > count {
            return Err(Error::Invalid);
        }
        Ok(Review {
            revision: revision.into(),
            base_sha256: self.base.sha256().into(),
            proposed_sha256,
            base_length: Offset(self.base.length()),
            proposed_length: Offset(self.length),
            original_bytes: Offset(original_bytes),
            proposed_bytes: Offset(proposed_bytes),
            range_count: count,
            next_range: (start + ranges.len() < count).then_some(start + ranges.len()),
            ranges,
            fingerprint: format!("{:x}", proof.finalize()),
        })
    }

    pub fn snapshot(&self) -> DraftState {
        DraftState {
            version: 1,
            nonce: self.nonce.clone(),
            epoch: self.epoch,
            base_sha256: self.base.sha256().into(),
            base_length: Offset(self.base.length()),
            cursor: self.cursor,
            commands: self
                .history
                .iter()
                .map(|command| {
                    command
                        .forward
                        .iter()
                        .map(|edit| Splice {
                            offset: Offset(edit.at),
                            delete_length: Offset(edit.expected.len() as u64),
                            expected_hex: encode_hex(&edit.expected),
                            insert_hex: encode_hex(&edit.inserted),
                        })
                        .collect()
                })
                .collect(),
        }
    }

    pub fn resume(base: B, state: DraftState) -> Result<Self, Error> {
        if state.version != 1
            || state.base_sha256 != base.sha256()
            || state.base_length.0 != base.length()
            || state.commands.len() > MAX_HISTORY
            || state.cursor > state.commands.len()
            || state.nonce.len() > 128
            || state.nonce.is_empty()
            || state.epoch < state.commands.len() as u64
        {
            return Err(Error::Invalid);
        }
        let mut draft = Self::new(base)?;
        for splices in state.commands {
            draft.patch(&PatchBatch {
                revision: draft.revision(),
                splices,
            })?;
        }
        while draft.cursor > state.cursor {
            draft.undo(&draft.revision())?;
        }
        draft.nonce = state.nonce;
        draft.epoch = state.epoch;
        Ok(draft)
    }
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 15) as usize] as char);
    }
    output
}

/// Canonical payload identity ignores hex letter case and independent splice order.
/// Current-document bounds and revision remain checked when applying the batch.
pub fn canonical(batch: &PatchBatch) -> Result<PatchBatch, Error> {
    let decoded = decode(&batch.splices, MAX_EDIT_SIZE)?;
    Ok(PatchBatch {
        revision: batch.revision.clone(),
        splices: decoded
            .iter()
            .map(|edit| Splice {
                offset: Offset(edit.at),
                delete_length: Offset(edit.expected.len() as u64),
                expected_hex: encode_hex(&edit.expected),
                insert_hex: encode_hex(&edit.inserted),
            })
            .collect(),
    })
}

fn decode(splices: &[Splice], length: u64) -> Result<Vec<Edit>, Error> {
    if splices.is_empty() || splices.len() > MAX_SPLICES {
        return Err(Error::Limit);
    }
    let mut budget = 0usize;
    let mut result = Vec::with_capacity(splices.len());
    for splice in splices {
        budget = budget
            .checked_add(splice.expected_hex.len())
            .and_then(|n| n.checked_add(splice.insert_hex.len()))
            .ok_or(Error::Limit)?;
        if budget > MAX_PATCH_BYTES * 2 {
            return Err(Error::Limit);
        }
        let expected = decode_hex(&splice.expected_hex, MAX_PATCH_BYTES)?;
        let inserted = decode_hex(&splice.insert_hex, MAX_PATCH_BYTES)?;
        if splice.delete_length.0 != expected.len() as u64
            || splice
                .offset
                .0
                .checked_add(splice.delete_length.0)
                .is_none_or(|end| end > length)
            || (expected.is_empty() && inserted.is_empty())
        {
            return Err(Error::Invalid);
        }
        result.push(Edit {
            at: splice.offset.0,
            expected: expected.into(),
            inserted: inserted.into(),
        });
    }
    result.sort_by_key(|edit| edit.at);
    for pair in result.windows(2) {
        if pair[0].at == pair[1].at || pair[0].at + pair[0].expected.len() as u64 > pair[1].at {
            return Err(Error::Invalid);
        }
    }
    Ok(result)
}

fn inverse(edits: &[Edit]) -> Result<Vec<Edit>, Error> {
    let mut shift = 0i64;
    let mut result = Vec::with_capacity(edits.len());
    for edit in edits {
        let at = edit.at.checked_add_signed(shift).ok_or(Error::Invalid)?;
        result.push(Edit {
            at,
            expected: edit.inserted.clone(),
            inserted: edit.expected.clone(),
        });
        shift += edit.inserted.len() as i64 - edit.expected.len() as i64;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Base {
        bytes: Vec<u8>,
        digest: String,
    }
    impl Base {
        fn new(bytes: Vec<u8>) -> Self {
            let digest = format!("{:x}", Sha256::digest(&bytes));
            Self { bytes, digest }
        }
    }
    impl ImmutableBase for Base {
        fn length(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn sha256(&self) -> &str {
            &self.digest
        }
        fn read(&self, at: u64, length: usize) -> Result<Vec<u8>, Error> {
            self.bytes
                .get(at as usize..at as usize + length)
                .map(<[u8]>::to_vec)
                .ok_or(Error::Io)
        }
    }
    fn edit(at: u64, expected: &[u8], inserted: &[u8]) -> Splice {
        Splice {
            offset: Offset(at),
            delete_length: Offset(expected.len() as u64),
            expected_hex: encode_hex(expected),
            insert_hex: encode_hex(inserted),
        }
    }
    fn patch(d: &mut Draft<Base>, splices: Vec<Splice>) -> Result<String, Error> {
        d.patch(&PatchBatch {
            revision: d.revision(),
            splices,
        })
    }
    fn bytes(d: &Draft<Base>) -> Vec<u8> {
        d.read(0, d.length().0 as usize).unwrap()
    }

    #[test]
    fn review_refuses_base_corruption_even_when_only_inserted_bytes_would_be_read() {
        let mut base = Base::new(b"original".to_vec());
        base.bytes.fill(0); // Keep the claimed digest and length unchanged.
        let mut draft = Draft::new(base).unwrap();
        patch(&mut draft, vec![edit(0, &[0; 8], b"replacement")]).unwrap();
        assert!(matches!(
            draft.review(&draft.revision(), 0, 128, || false),
            Err(Error::Changed)
        ));
    }

    #[test]
    fn a_batch_uses_original_coordinates_and_undo_redo_are_atomic_and_fenced() {
        let mut d = Draft::new(Base::new((0..=255).collect())).unwrap();
        let original = d.revision();
        let changed = patch(
            &mut d,
            vec![
                edit(250, &[250, 251, 252, 253, 254, 255], &[]),
                edit(1, &[1, 2], &[255, 0, 128, 27]),
                edit(0, &[], b"head"),
            ],
        )
        .unwrap();
        let mut expected = b"head".to_vec();
        expected.push(0);
        expected.extend([255, 0, 128, 27]);
        expected.extend(3..250);
        assert_eq!(bytes(&d), expected);
        assert_eq!(d.length().0, 256);
        assert!(d.page(&original, Offset(0), 1).is_err());
        let undone = d.undo(&changed).unwrap();
        assert_eq!(bytes(&d), (0..=255).collect::<Vec<u8>>());
        assert_ne!(undone, original);
        assert!(!d.can_undo());
        assert!(d.can_redo());
        d.redo(&undone).unwrap();
        assert_eq!(bytes(&d), expected);
        let state = d.snapshot();
        let restored = Draft::resume(Base::new((0..=255).collect()), state).unwrap();
        assert_eq!(restored.revision(), d.revision());
        assert_eq!(bytes(&restored), expected);
    }
    #[test]
    fn an_invalid_later_splice_changes_neither_bytes_revision_nor_history() {
        let mut d = Draft::new(Base::new(b"abcdef".to_vec())).unwrap();
        let revision = d.revision();
        for splices in [
            vec![edit(0, b"a", b"A"), edit(4, b"wrong", b"E")],
            vec![edit(0, b"ab", b"A"), edit(1, b"b", b"B")],
            vec![edit(0, b"", b"A"), edit(0, b"", b"B")],
            vec![Splice {
                offset: Offset(u64::MAX),
                ..edit(0, b"a", b"A")
            }],
            vec![Splice {
                expected_hex: "0".into(),
                ..edit(0, b"a", b"A")
            }],
            vec![Splice {
                insert_hex: "gg".into(),
                ..edit(0, b"a", b"A")
            }],
            vec![Splice {
                delete_length: Offset(2),
                ..edit(0, b"a", b"A")
            }],
            vec![edit(0, b"", b"")],
            vec![],
            vec![edit(0, b"", b"X"); MAX_SPLICES + 1],
            vec![edit(0, b"", &vec![0; MAX_PATCH_BYTES + 1])],
        ] {
            assert!(patch(&mut d, splices).is_err());
            assert_eq!(d.revision(), revision);
            assert_eq!(bytes(&d), b"abcdef");
            assert!(!d.can_undo());
            assert!(!d.can_redo());
        }
        assert!(
            d.patch(&PatchBatch {
                revision: "old".into(),
                splices: vec![edit(0, b"a", b"A")]
            })
            .is_err()
        );
        assert_eq!(d.revision(), revision);
    }
    #[test]
    fn adjacent_deletes_and_insertions_round_trip_without_coordinate_drift() {
        let mut d = Draft::new(Base::new(b"abcd".to_vec())).unwrap();
        patch(
            &mut d,
            vec![
                edit(0, b"a", b""),
                edit(1, b"b", b""),
                edit(2, b"", b"XY"),
                edit(3, b"d", b"!"),
            ],
        )
        .unwrap();
        assert_eq!(bytes(&d), b"XYc!");
        d.undo(&d.revision()).unwrap();
        assert_eq!(bytes(&d), b"abcd");
        d.redo(&d.revision()).unwrap();
        assert_eq!(bytes(&d), b"XYc!");
    }
    #[test]
    fn empty_eof_history_branch_and_snapshot_keep_lossless_state() {
        let mut d = Draft::new(Base::new(vec![])).unwrap();
        patch(&mut d, vec![edit(0, b"", &[0, 255, 128])]).unwrap();
        let page = d.page(&d.revision(), Offset(3), 1).unwrap();
        assert!(page.eof && page.bytes.is_empty());
        d.undo(&d.revision()).unwrap();
        let state: DraftState =
            serde_json::from_slice(&serde_json::to_vec(&d.snapshot()).unwrap()).unwrap();
        let mut resumed = Draft::resume(Base::new(vec![]), state).unwrap();
        assert_eq!(resumed.revision(), d.revision());
        resumed.redo(&resumed.revision()).unwrap();
        assert_eq!(bytes(&resumed), [0, 255, 128]);
        resumed.undo(&resumed.revision()).unwrap();
        patch(&mut resumed, vec![edit(0, b"", b"branch")]).unwrap();
        assert!(!resumed.can_redo());
        assert_eq!(bytes(&resumed), b"branch");
        let mut state = resumed.snapshot();
        state.base_sha256 = "00".repeat(32);
        assert!(Draft::resume(Base::new(vec![]), state).is_err());
    }
    #[test]
    fn deterministic_edit_history_matches_a_byte_vector_reference() {
        let mut seed = 0x123456789abcdefu64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let base: Vec<u8> = (0..=255).collect();
        let mut d = Draft::new(Base::new(base.clone())).unwrap();
        let mut states = vec![base];
        let mut cursor = 0;
        for _ in 0..400 {
            let action = next() % 5;
            if action == 0 && d.can_undo() {
                d.undo(&d.revision()).unwrap();
                cursor -= 1;
            } else if action == 1 && d.can_redo() {
                d.redo(&d.revision()).unwrap();
                cursor += 1;
            } else {
                let mut reference = states[cursor].clone();
                let at = (next() as usize) % (reference.len() + 1);
                let delete = (next() as usize % 8).min(reference.len() - at);
                let insert: Vec<u8> = (0..(1 + next() % 8)).map(|_| next() as u8).collect();
                patch(
                    &mut d,
                    vec![edit(at as u64, &reference[at..at + delete], &insert)],
                )
                .unwrap();
                reference.splice(at..at + delete, insert);
                states.truncate(cursor + 1);
                states.push(reference);
                cursor += 1;
            }
            assert_eq!(bytes(&d), states[cursor]);
        }
    }
    #[test]
    fn review_pages_cover_original_coordinates_and_share_one_complete_fingerprint() {
        let mut d = Draft::new(Base::new(b"abcdef".to_vec())).unwrap();
        patch(&mut d, vec![edit(0, b"a", b"XY"), edit(4, b"e", b"Z")]).unwrap();
        let first = d.review(&d.revision(), 0, 1, || false).unwrap();
        let second = d.review(&d.revision(), 1, 1, || false).unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_eq!(first.range_count, 2);
        assert_eq!(first.next_range, Some(1));
        assert_eq!(second.next_range, None);
        assert_eq!(first.ranges[0].original_offset.0, 0);
        assert_eq!(first.ranges[0].draft_length.0, 2);
        assert_eq!(second.ranges[0].original_offset.0, 4);
        assert_eq!(second.ranges[0].draft_offset.0, 5);
        assert_eq!(
            first.proposed_sha256,
            format!("{:x}", Sha256::digest(b"XYbcdZf"))
        );
        assert_eq!(first.original_bytes.0, 2);
        assert_eq!(first.proposed_bytes.0, 3);
        assert!(matches!(
            d.review(&d.revision(), 0, 1, || true),
            Err(Error::Cancelled)
        ));
        let prior = first.fingerprint;
        d.undo(&d.revision()).unwrap();
        assert_ne!(
            d.review(&d.revision(), 0, 1, || false).unwrap().fingerprint,
            prior
        );
    }
    #[test]
    fn unchanged_overwrites_are_not_fabricated_changes_and_large_previews_stay_bounded() {
        let mut d = Draft::new(Base::new(vec![0; 1024])).unwrap();
        patch(&mut d, vec![edit(0, &[0], &[0])]).unwrap();
        let clean = d.review(&d.revision(), 0, 128, || false).unwrap();
        assert_eq!(clean.range_count, 0);
        assert_eq!(clean.base_sha256, clean.proposed_sha256);
        patch(&mut d, vec![edit(0, &vec![0; 512], &vec![255; 512])]).unwrap();
        let review = d.review(&d.revision(), 0, 128, || false).unwrap();
        assert_eq!(review.range_count, 1);
        assert!(review.ranges[0].preview_truncated);
        assert_eq!(review.ranges[0].before_hex.len(), 512);
        assert_eq!(review.ranges[0].after_hex.len(), 512);
        let mut output = Vec::new();
        assert_eq!(
            d.write_to(&mut output, || false).unwrap(),
            review.proposed_sha256
        );
        assert_eq!(output.len(), 1024);
        assert_eq!(&output[..512], &[255; 512]);
    }
    #[test]
    fn history_capacity_refuses_without_silently_discarding_undo() {
        let mut d = Draft::new(Base::new(vec![])).unwrap();
        for _ in 0..MAX_HISTORY {
            let end = d.length().0;
            patch(&mut d, vec![edit(end, b"", b"x")]).unwrap();
        }
        let revision = d.revision();
        assert!(patch(&mut d, vec![edit(0, b"", b"y")]).is_err());
        assert_eq!(d.revision(), revision);
        d.undo(&revision).unwrap();
        patch(&mut d, vec![edit(0, b"", b"y")]).unwrap();
        assert!(!d.can_redo());
    }
}
