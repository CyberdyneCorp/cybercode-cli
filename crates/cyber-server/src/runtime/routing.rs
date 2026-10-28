//! Who a model request is for (`provider-catalog` → Session routing headers and Responses
//! request shaping): the root Session, the prompt cache key derived from it, and the
//! sticky-routing state the requests of one Turn share.

use cyber_llm::{LlmRequest, Routing, TurnState};
use rusqlite::{OptionalExtension, params};

use super::{Inner, SessionInfo};

/// How far up a parent chain the root lookup walks; a deeper or cyclic chain stops there.
const MAX_DEPTH: i64 = 64;

/// Who a request is for: its routing and its prompt cache key.
pub(crate) struct Address {
    pub routing: Routing,
    pub cache_key: String,
}

impl Address {
    pub(crate) fn apply(self, request: &mut LlmRequest) {
        request.cache_key = Some(self.cache_key);
        request.routing = Some(self.routing);
    }
}

impl Inner {
    /// The address of a request `info`'s Session makes: `purpose` names a call outside a
    /// Turn (`compact`, `title`, `recap`); a subagent's Turns carry its agent name. The
    /// prompt cache key is `<root>` for a root Session and `<root>:<agent>` for a
    /// subagent, so a Session's subagents share one cache per agent; it is sent on every
    /// request, a Turn's or not.
    pub(crate) fn address_of(&self, info: &SessionInfo, purpose: Option<&str>) -> Address {
        let subagent = purpose
            .map(str::to_owned)
            .or_else(|| info.parent_id.as_ref().map(|_| info.agent.clone()));
        let root = self.root_id(info);
        let cache_key = match &info.parent_id {
            Some(_) => format!("{root}:{}", info.agent),
            None => root.clone(),
        };
        Address {
            routing: Routing {
                root,
                session: info.id.clone(),
                subagent,
                turn_state: None,
            },
            cache_key,
        }
    }

    /// Address a Turn's `request` from `info`'s Session, with the Turn's sticky-routing
    /// state.
    pub(crate) fn address(
        &self,
        request: &mut LlmRequest,
        info: &SessionInfo,
        turn_state: Option<TurnState>,
    ) {
        let mut address = self.address_of(info, None);
        address.routing.turn_state = turn_state;
        address.apply(request);
    }

    /// The ID of the Session at the top of `info`'s parent chain. Read from the store, so
    /// no other Session's lock is taken while the caller holds this one's.
    fn root_id(&self, info: &SessionInfo) -> String {
        let Some(parent) = info.parent_id.clone() else {
            return info.id.clone();
        };
        let fallback = parent.clone();
        self.store
            .read(move |db| {
                Ok(db
                    .query_row(
                        "WITH RECURSIVE up(id, parent, depth) AS (
                            SELECT id, parent_id, 0 FROM session WHERE id = ?1
                            UNION ALL
                            SELECT s.id, s.parent_id, up.depth + 1 FROM up
                            JOIN session s ON s.id = up.parent WHERE up.depth < ?2
                        ) SELECT id FROM up ORDER BY depth DESC LIMIT 1",
                        params![parent, MAX_DEPTH],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?)
            })
            .ok()
            .flatten()
            .unwrap_or(fallback)
    }
}
