//! Shared bounded literal search over live sources and immutable draft revisions.
use super::*;
use std::time::{Duration, Instant};
pub(super) fn bounded(
    request: &SearchRequest,
    length: u64,
    source: Source,
    mut read: impl FnMut(u64, usize) -> Result<Vec<u8>, Error>,
    cancelled: impl Fn() -> bool,
) -> Result<SearchResult, Error> {
    let pattern = decode_hex(&request.hex, MAX_PATTERN)?;
    if pattern.is_empty() || request.offset.0 > length {
        return Err(Error::Invalid);
    }
    let matcher = aho_corasick::AhoCorasick::new([&pattern]).map_err(|_| Error::Invalid)?;
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut at = request.offset.0;
    let mut carry = Vec::new();
    let mut offsets = Vec::new();
    let mut scanned = 0usize;
    let mut next = None;
    while at < length && scanned < MAX_SEARCH_BYTES {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let remaining = usize::try_from(length - at).unwrap_or(usize::MAX);
        let chunk = read(
            at,
            DEFAULT_PAGE.min(MAX_SEARCH_BYTES - scanned).min(remaining),
        )?;
        if chunk.is_empty() {
            return Err(Error::Changed);
        }
        let start = at - carry.len() as u64;
        scanned += chunk.len();
        at += chunk.len() as u64;
        carry.extend_from_slice(&chunk);
        for found in matcher.find_overlapping_iter(&carry) {
            offsets.push(Offset(start + found.start() as u64));
            if offsets.len() == MAX_MATCHES {
                next = Some(Offset(start + found.start() as u64 + 1));
                break;
            }
        }
        if next.is_some() {
            break;
        }
        let keep = (pattern.len() - 1).min(carry.len());
        carry.drain(..carry.len() - keep);
        if Instant::now() >= deadline {
            break;
        }
    }
    if next.is_none() && at < length {
        next = Some(Offset(at.saturating_sub(carry.len() as u64)));
    }
    Ok(SearchResult {
        revision: request.revision.clone(),
        source,
        offsets,
        next_offset: next,
        scanned: Offset(scanned as u64),
    })
}
