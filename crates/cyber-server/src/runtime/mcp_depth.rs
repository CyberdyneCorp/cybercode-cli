//! Run identity and the MCP serve recursion depth of each run (`mcp-server` → Recursion
//! guard). A run is one Drain of a Session, from its start until the Session is idle; its
//! `mcp_depth` is the depth of the MCP input that started it, raised by deeper MCP input
//! admitted into it or carried by a cross-session message promoted into it, and 0 for a
//! run nothing deeper joined (user input). Depths live in memory; a Drain that continues
//! a run after a restart restores the depth of the messages promoted into it.

use std::sync::PoisonError;

use tokio_util::sync::CancellationToken;

use super::{Inner, Runtime};

/// Ancestors examined at most when walking a Session's parents.
const MAX_ANCESTORS: usize = 64;

/// One run (Drain) of a Session; equal only to the same run, never to a later one.
#[derive(Clone, PartialEq, Eq)]
pub struct RunMarker(CancellationToken);

impl std::fmt::Debug for RunMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RunMarker")
    }
}

impl Inner {
    /// Raise the `mcp_depth` of the Session's running Drain to at least `depth`; a no-op
    /// when the Session is idle. Promotion calls it for received messages, so a message
    /// raises the run that reads it, never the run it was admitted into.
    pub(crate) fn raise_mcp_depth(&self, session_id: &str, depth: u32) {
        let Some(run) = self.run_token(session_id) else {
            return;
        };
        let mut depths = self
            .mcp_depths
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let entry = depths
            .entry(session_id.to_owned())
            .or_insert_with(|| (run.clone(), 0));
        if entry.0 != run {
            *entry = (run, 0);
        }
        entry.1 = entry.1.max(depth);
    }
}

impl Runtime {
    /// The Session's running Drain, if any.
    pub fn run_marker(&self, session_id: &str) -> Option<RunMarker> {
        self.inner
            .drains
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session_id)
            .map(|entry| RunMarker(entry.cancel.clone()))
    }

    /// Raise the `mcp_depth` of the Session's running Drain to at least `depth`; a no-op
    /// when the Session is idle.
    pub fn raise_mcp_depth(&self, session_id: &str, depth: u32) {
        self.inner.raise_mcp_depth(session_id, depth);
    }

    /// The `mcp_depth` of the Session's running Drain (0 when idle or when no MCP input
    /// joined it).
    pub fn run_mcp_depth(&self, session_id: &str) -> u32 {
        let Some(RunMarker(run)) = self.run_marker(session_id) else {
            return 0;
        };
        self.inner
            .mcp_depths
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session_id)
            .filter(|(token, _)| token == &run)
            .map_or(0, |(_, depth)| *depth)
    }

    /// The Session's parent chain, nearest first (bounded, cycle-safe).
    pub fn ancestor_ids(&self, session_id: &str) -> Vec<String> {
        let mut ancestors: Vec<String> = Vec::new();
        let mut current = session_id.to_owned();
        while ancestors.len() < MAX_ANCESTORS {
            let Some(parent) = self.parent_id(&current) else {
                break;
            };
            if parent == session_id || ancestors.contains(&parent) {
                break;
            }
            ancestors.push(parent.clone());
            current = parent;
        }
        ancestors
    }

    /// The depth a process started for the Session runs at: the deepest running
    /// `mcp_depth` of the Session and its ancestors (a subagent runs inside its
    /// parent's run).
    pub fn effective_mcp_depth(&self, session_id: &str) -> u32 {
        std::iter::once(session_id.to_owned())
            .chain(self.ancestor_ids(session_id))
            .map(|id| self.run_mcp_depth(&id))
            .max()
            .unwrap_or(0)
    }

    fn parent_id(&self, session_id: &str) -> Option<String> {
        let id = session_id.to_owned();
        self.inner
            .store
            .read(move |conn| {
                use rusqlite::OptionalExtension;
                Ok(conn
                    .query_row("SELECT parent_id FROM session WHERE id = ?1", [id], |r| {
                        r.get::<_, Option<String>>(0)
                    })
                    .optional()?
                    .flatten())
            })
            .ok()
            .flatten()
    }
}
