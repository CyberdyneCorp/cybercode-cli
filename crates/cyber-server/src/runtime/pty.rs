//! PTY sessions (`background-tasks` → PTY sessions, Model interaction with PTYs): the
//! retained output of a PTY Job, its input, and the single-use tickets clients redeem to
//! attach over a WebSocket. The tool layer owns the terminal and its process; the runtime
//! keeps the newest [`BUFFER_BYTES`] of output with a byte cursor so a client can replay,
//! resume or tail it.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::time::Instant;

use super::{Runtime, RuntimeError, WeakRuntime};

/// Output retained per PTY.
pub const BUFFER_BYTES: usize = 2 * 1024 * 1024;
/// How long an attach ticket stays redeemable.
pub const TICKET_TTL: Duration = Duration::from_secs(60);
/// Live chunks a slow subscriber may fall behind before it resynchronizes from the buffer.
const LIVE_CHUNKS: usize = 1024;

/// Writes input to the PTY's terminal; an error means the terminal is gone.
pub type PtyInput = Box<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync>;

/// A live update of a PTY.
#[derive(Debug, Clone)]
pub enum PtyChunk {
    /// `bytes` start at byte `cursor` of the PTY's output.
    Output { cursor: u64, bytes: Arc<[u8]> },
    /// The program exited; no more output follows.
    Closed,
}

/// Where an attach starts: the cursor query parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyCursor {
    /// Omitted: replay the retained buffer.
    Replay,
    /// `-1`: only output from now on.
    Tail,
    /// From this byte, or from the first retained one when it is older.
    At(u64),
}

impl PtyCursor {
    /// `None` replays, `-1` tails, other negatives are refused.
    pub fn parse(raw: Option<i64>) -> Result<Self, RuntimeError> {
        match raw {
            None => Ok(Self::Replay),
            Some(-1) => Ok(Self::Tail),
            Some(n) => u64::try_from(n)
                .map(Self::At)
                .map_err(|_| RuntimeError::Invalid("cursor must be -1 or at least 0".into())),
        }
    }
}

/// What an attach sees first: the replayed bytes from `start`, then `live`.
pub struct PtyAttach {
    /// The cursor of the first replayed byte.
    pub start: u64,
    pub replay: Vec<u8>,
    /// The program already exited; `replay` is everything there is.
    pub closed: bool,
    pub live: broadcast::Receiver<PtyChunk>,
}

impl PtyAttach {
    /// The cursor just after the replayed bytes.
    pub fn end(&self) -> u64 {
        self.start + self.replay.len() as u64
    }
}

#[derive(Default)]
struct Ring {
    bytes: VecDeque<u8>,
    /// The cursor of `bytes[0]`.
    start: u64,
    closed: bool,
}

impl Ring {
    fn end(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }

    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        let over = self.bytes.len().saturating_sub(BUFFER_BYTES);
        if over > 0 {
            self.bytes.drain(..over);
            self.start += over as u64;
        }
    }

    fn from(&self, cursor: u64) -> Vec<u8> {
        let skip = cursor.clamp(self.start, self.end()) - self.start;
        self.bytes
            .iter()
            .skip(usize::try_from(skip).unwrap_or(usize::MAX))
            .copied()
            .collect()
    }
}

/// What the tool that owns a terminal registers.
pub struct PtySpec {
    pub session_id: String,
    pub job_id: String,
    /// The command it runs, as authorized.
    pub command: String,
    pub cols: u16,
    pub rows: u16,
    pub input: PtyInput,
}

/// One running PTY.
pub struct Pty {
    pub job_id: String,
    pub session_id: String,
    /// The command it runs, as authorized.
    pub command: String,
    pub cols: u16,
    pub rows: u16,
    ring: Mutex<Ring>,
    live: broadcast::Sender<PtyChunk>,
    input: PtyInput,
    /// Where the model's next `pty_read` continues.
    read_cursor: AtomicU64,
    /// The model's typed, not yet submitted line, held while its input is authorized.
    typed: tokio::sync::Mutex<Vec<u8>>,
}

impl Pty {
    fn ring(&self) -> std::sync::MutexGuard<'_, Ring> {
        self.ring.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The retained output from `cursor` and a receiver for what follows, taken together
    /// so nothing is missed or repeated.
    pub fn attach(&self, cursor: PtyCursor) -> PtyAttach {
        let ring = self.ring();
        let live = self.live.subscribe();
        let start = match cursor {
            PtyCursor::Replay => ring.start,
            PtyCursor::Tail => ring.end(),
            PtyCursor::At(n) => n.clamp(ring.start, ring.end()),
        };
        PtyAttach {
            start,
            replay: ring.from(start),
            closed: ring.closed,
            live,
        }
    }

    /// The cursor after the last output byte.
    pub fn end(&self) -> u64 {
        self.ring().end()
    }

    pub fn closed(&self) -> bool {
        self.ring().closed
    }

    /// The model's typed line so far; holding it serializes the model's writes.
    pub async fn typed(&self) -> tokio::sync::MutexGuard<'_, Vec<u8>> {
        self.typed.lock().await
    }

    /// Send input to the program.
    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        if self.closed() {
            return Err(format!("PTY {} has exited", self.job_id));
        }
        (self.input)(bytes)
    }

    /// Where the model's next read continues.
    pub fn read_cursor(&self) -> u64 {
        self.read_cursor.load(Ordering::SeqCst)
    }

    pub fn set_read_cursor(&self, cursor: u64) {
        self.read_cursor.store(cursor, Ordering::SeqCst);
    }
}

/// The PTY's output side, held by the tool that owns the terminal. Dropping it marks the
/// PTY exited and unregisters it.
pub struct PtyFeed {
    pty: Arc<Pty>,
    runtime: WeakRuntime,
}

impl PtyFeed {
    pub fn pty(&self) -> &Arc<Pty> {
        &self.pty
    }

    /// Retain and publish program output.
    pub fn push(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut ring = self.pty.ring();
        let cursor = ring.end();
        ring.push(bytes);
        let _ = self.pty.live.send(PtyChunk::Output {
            cursor,
            bytes: Arc::from(bytes),
        });
    }
}

impl Drop for PtyFeed {
    fn drop(&mut self) {
        self.pty.ring().closed = true;
        let _ = self.pty.live.send(PtyChunk::Closed);
        if let Some(runtime) = self.runtime.upgrade() {
            runtime.inner.ptys.remove(&self.pty.job_id);
        }
    }
}

struct Ticket {
    job_id: String,
    expires: Instant,
}

/// The running PTYs and the outstanding attach tickets.
#[derive(Default)]
pub(super) struct Registry {
    ptys: Mutex<HashMap<String, Arc<Pty>>>,
    tickets: Mutex<HashMap<String, Ticket>>,
}

impl Registry {
    fn remove(&self, job_id: &str) {
        self.ptys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(job_id);
        self.tickets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, ticket| ticket.job_id != job_id);
    }
}

impl Runtime {
    /// Register the PTY of a running Job; its output goes through the returned feed.
    pub fn register_pty(&self, spec: PtySpec) -> PtyFeed {
        let pty = Arc::new(Pty {
            job_id: spec.job_id,
            session_id: spec.session_id,
            command: spec.command,
            cols: spec.cols,
            rows: spec.rows,
            ring: Mutex::default(),
            live: broadcast::channel(LIVE_CHUNKS).0,
            input: spec.input,
            read_cursor: AtomicU64::new(0),
            typed: tokio::sync::Mutex::default(),
        });
        self.inner
            .ptys
            .ptys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(pty.job_id.clone(), Arc::clone(&pty));
        PtyFeed {
            pty,
            runtime: self.downgrade(),
        }
    }

    /// The running PTY of a Job.
    pub fn pty(&self, job_id: &str) -> Option<Arc<Pty>> {
        self.inner
            .ptys
            .ptys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(job_id)
            .cloned()
    }

    /// A single-use ticket to attach to a running PTY within [`TICKET_TTL`].
    pub fn issue_pty_ticket(&self, job_id: &str) -> Result<String, RuntimeError> {
        if self.pty(job_id).is_none() {
            return Err(RuntimeError::Invalid(format!("No running PTY {job_id}")));
        }
        let ticket = cyber_core::ids::new_id("ptyt");
        let now = Instant::now();
        let mut tickets = self
            .inner
            .ptys
            .tickets
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        tickets.retain(|_, ticket| ticket.expires > now);
        tickets.insert(
            ticket.clone(),
            Ticket {
                job_id: job_id.into(),
                expires: now + TICKET_TTL,
            },
        );
        Ok(ticket)
    }

    /// Redeem `ticket` for `job_id`: true once, before it expires, for the PTY it was
    /// issued for. Any redemption attempt consumes the ticket.
    pub fn redeem_pty_ticket(&self, ticket: &str, job_id: &str) -> bool {
        let taken = self
            .inner
            .ptys
            .tickets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(ticket);
        taken.is_some_and(|t| t.job_id == job_id && t.expires > Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_the_newest_bytes_and_moves_its_start() {
        let mut ring = Ring::default();
        ring.push(&vec![b'a'; BUFFER_BYTES]);
        ring.push(b"xyz");
        assert_eq!(ring.start, 3);
        assert_eq!(ring.end(), BUFFER_BYTES as u64 + 3);
        assert_eq!(ring.from(ring.end() - 3), b"xyz");
        assert_eq!(ring.from(0).len(), BUFFER_BYTES);
        assert!(ring.from(u64::MAX).is_empty());
    }

    #[test]
    fn cursor_parses_replay_tail_and_offsets() {
        assert_eq!(PtyCursor::parse(None).unwrap(), PtyCursor::Replay);
        assert_eq!(PtyCursor::parse(Some(-1)).unwrap(), PtyCursor::Tail);
        assert_eq!(PtyCursor::parse(Some(7)).unwrap(), PtyCursor::At(7));
        assert!(PtyCursor::parse(Some(-2)).is_err());
    }
}
