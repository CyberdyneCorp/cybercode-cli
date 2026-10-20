//! Message addressing (`cross-session-messaging` → Addressing, Reachable session listing).
//!
//! An address is resolved on the server, relative to the caller (a Session, or none for a
//! client), in tiers: an exact `ses_` ID of a reachable Session; any address `X@P` (a
//! Session `X` of the configured peer `P`, which [`super::Messaging::resolve`] looks up in
//! `peers.remotes`; `PeerNotConfiguredError` without one); then names scoped
//! to the caller (for a team member, `lead`, `all` and its teammates' names; then its
//! subagents by `subagent_name`); then the exact title of a non-archived root Session.
//! The first tier with a match decides; several matches in it are ambiguous. No ID
//! prefixes and no fuzzy matches resolve, so a typo never reaches the wrong Session.
//! `all` names every other member of the caller's team that is not `stopped`; a send to
//! it fans out to each of them ([`team_all`]).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::inbound::{InboundOverride, SenderAddresses};

/// The most entries a reachable listing returns.
pub const REACHABLE_LIMIT: u32 = 200;

/// How the caller reaches a Session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AddressKind {
    /// A child Session of the caller.
    Subagent,
    /// A member of the caller's team (`agent-teams`).
    Teammate,
    /// Another root Session on this server.
    Local,
    /// A Session of a configured peer (`X@P`), resolved by that peer.
    Peer,
}

/// Where a Session runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionLocation {
    pub directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

/// One Session an address matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Candidate {
    pub session_id: String,
    pub name: String,
    pub kind: AddressKind,
    pub location: SessionLocation,
}

/// A resolved target: the Session, and the address shown in records (the name used when
/// it resolved uniquely, else the ID).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub address: String,
    pub candidate: Candidate,
    pub archived: bool,
    /// The target's Session-level `messaging.inbound` override.
    pub inbound: Option<InboundOverride>,
    /// The peer of an `X@P` target. Its candidate is the address itself until the peer
    /// resolves `X`: `session_id` and `name` are `X@P`, which `message.send` checks.
    pub peer: Option<String>,
}

impl Target {
    /// The target of `session@peer`, before the peer resolves `session`.
    pub fn remote(session: &str, peer: &str) -> Self {
        let address = format!("{session}@{peer}");
        Self {
            address: address.clone(),
            candidate: Candidate {
                session_id: address.clone(),
                name: address,
                kind: AddressKind::Peer,
                location: SessionLocation {
                    directory: String::new(),
                    workspace: None,
                },
            },
            archived: false,
            inbound: None,
            peer: Some(peer.to_owned()),
        }
    }
}

/// Why an address resolves to no target. Nothing is recorded for any of these.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("AddressNotFoundError: no reachable Session is addressed by `{0}`")]
    NotFound(String),
    #[error("AmbiguousAddressError: `{address}` matches {} Sessions", candidates.len())]
    Ambiguous {
        address: String,
        candidates: Vec<Candidate>,
    },
    /// Reserved for the Relay transport (P3): an account-routed address no configured peer
    /// matches. Direct peers fail with `PeerNotConfiguredError` instead.
    #[error("AccountRequiredError: `{0}` names a remote Session; no remote transport is available")]
    AccountRequired(String),
    #[error("InvalidRequestError: Cannot send a message to the current Session")]
    SelfTarget,
}

/// A parsed address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// An exact `ses_` ID; when no reachable Session has it, it is tried as a name.
    Id(String),
    Name(String),
    /// `session@peer`: a Session the peer resolves with its own tiers.
    Remote {
        session: String,
        peer: String,
    },
}

/// Parse `to`: a leading `@` is stripped once (a mention still addresses a name), and a
/// remaining `@` names a Session of a peer (`X@P`). `X` must be a local address there (no
/// further `@`) and `P` a peer name; anything else addresses nothing.
pub fn parse(to: &str) -> Result<Address, AddressError> {
    let address = to.strip_prefix('@').unwrap_or(to);
    if address.is_empty() {
        return Err(AddressError::NotFound(to.to_owned()));
    }
    if let Some((session, peer)) = address.rsplit_once('@') {
        let remote = !session.is_empty()
            && !session.contains('@')
            && cyber_core::config::valid_peer_name(peer);
        if !remote {
            return Err(AddressError::NotFound(address.to_owned()));
        }
        return Ok(Address::Remote {
            session: session.to_owned(),
            peer: peer.to_owned(),
        });
    }
    if address.starts_with("ses_") {
        return Ok(Address::Id(address.to_owned()));
    }
    Ok(Address::Name(address.to_owned()))
}

/// Whether `name` is what a caller would type to reach a Session of that name.
pub fn addressable_name(name: &str) -> bool {
    matches!(parse(name), Ok(Address::Name(parsed)) if parsed == name)
}

/// The first non-empty tier decides; several matches in it are ambiguous.
pub fn pick(address: &str, tiers: Vec<Vec<Target>>) -> Result<Target, AddressError> {
    let Some(mut matches) = tiers.into_iter().find(|tier| !tier.is_empty()) else {
        return Err(AddressError::NotFound(address.to_owned()));
    };
    if matches.len() > 1 {
        return Err(AddressError::Ambiguous {
            address: address.to_owned(),
            candidates: matches.into_iter().map(|t| t.candidate).collect(),
        });
    }
    Ok(matches.remove(0))
}

/// Resolve `to` from `caller` against the store; the outer error is the store's. A
/// remote address (`X@P`) names nothing here: [`super::Messaging::resolve`] sends it to
/// the peer, and a peer's own request may not name a further peer.
pub fn resolve(
    db: &Connection,
    caller: Option<&str>,
    to: &str,
) -> rusqlite::Result<Result<Target, AddressError>> {
    let address = match parse(to) {
        Ok(address) => address,
        Err(e) => return Ok(Err(e)),
    };
    let found = match address {
        Address::Id(id) if caller == Some(id.as_str()) => Err(AddressError::SelfTarget),
        Address::Id(id) => match by_id(db, caller, &id)? {
            Some(target) => Ok(target),
            None => pick(&id, name_tiers(db, caller, &id)?),
        },
        Address::Name(name) => pick(&name, name_tiers(db, caller, &name)?),
        Address::Remote { .. } => Err(AddressError::NotFound(to.to_owned())),
    };
    Ok(found.and_then(|target| {
        if caller == Some(target.candidate.session_id.as_str()) {
            Err(AddressError::SelfTarget)
        } else {
            Ok(target)
        }
    }))
}

const COLUMNS: &str =
    "id, title, directory, parent_id, subagent_name, archived, inbound, inbound_reason";
/// The column after [`COLUMNS`] in a query that selects more.
const NEXT_COLUMN: usize = 8;

/// A Session row as a target of `caller`: a child of the caller is a subagent, named by
/// its `subagent_name` when it has one.
fn target(row: &rusqlite::Row<'_>, caller: Option<&str>) -> rusqlite::Result<Target> {
    let (id, title, directory): (String, String, String) = (row.get(0)?, row.get(1)?, row.get(2)?);
    let parent: Option<String> = row.get(3)?;
    let subagent: Option<String> = row.get(4)?;
    let child = parent.is_some() && parent.as_deref() == caller;
    let kind = if child {
        AddressKind::Subagent
    } else {
        AddressKind::Local
    };
    let name = if child {
        subagent.unwrap_or(title)
    } else {
        title
    };
    Ok(Target {
        address: id.clone(),
        candidate: Candidate {
            session_id: id,
            name,
            kind,
            location: SessionLocation {
                directory,
                workspace: None,
            },
        },
        archived: row.get::<_, i64>(5)? != 0,
        inbound: inbound_override(row.get(6)?, row.get(7)?),
        peer: None,
    })
}

/// The override the `inbound` columns hold. A value this build cannot read refuses, so
/// an unknown policy never delivers.
fn inbound_override(inbound: Option<String>, reason: Option<String>) -> Option<InboundOverride> {
    let inbound = inbound?;
    let parsed = serde_json::from_value(serde_json::json!({"inbound": inbound, "reason": reason}));
    Some(parsed.unwrap_or(InboundOverride {
        inbound: cyber_core::config::InboundPolicy::Refuse,
        reason: None,
    }))
}

/// The deepest ancestor chain [`sender_addresses`] walks.
const MAX_CHAIN: u32 = 64;

/// Whether a unique root title may be a trusted sender address: only a name a caller
/// would type for a Session, never one shaped like another Session's ID, a remote
/// address or a client's address (`client`, `client:<label>`, `mcp:<label>`). Titles are
/// free text (set with PATCH, or generated from message content), so any other title
/// only restricts.
fn trusted_title(title: &str) -> bool {
    addressable_name(title)
        && title != "client"
        && !title.starts_with("client:")
        && !title.starts_with("mcp:")
}

/// The addresses inbound patterns match for a local sender Session. Trusted: its ID, and
/// its title when it is a root Session no other non-archived root shares and the title is
/// a plain name ([`trusted_title`]). Every other name it has (a shared title, its subagent
/// name) and its ancestors' IDs and names only restrict, so a subagent cannot shed its
/// root's `refuse` match and no Session can borrow another's allowlisted name.
pub fn sender_addresses(db: &Connection, sender: &str) -> rusqlite::Result<SenderAddresses> {
    let mut stmt = db.prepare(
        "WITH RECURSIVE chain(id, parent_id, title, subagent_name, depth) AS (
            SELECT id, parent_id, title, subagent_name, 0 FROM session WHERE id = ?1
            UNION ALL
            SELECT s.id, s.parent_id, s.title, s.subagent_name, chain.depth + 1
              FROM session s JOIN chain ON s.id = chain.parent_id
             WHERE chain.depth < ?2)
         SELECT id, title, subagent_name, parent_id IS NULL AND NOT EXISTS (
                  SELECT 1 FROM session o WHERE o.parent_id IS NULL AND o.archived = 0
                     AND o.title = chain.title AND o.id <> chain.id)
           FROM chain ORDER BY depth",
    )?;
    let chain = stmt
        .query_map(params![sender, MAX_CHAIN], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut addresses = SenderAddresses::trusted(vec![sender.to_owned()]);
    for (depth, (id, title, subagent, unique_root)) in chain.into_iter().enumerate() {
        if depth == 0 && unique_root && trusted_title(&title) {
            addresses.trusted.push(title);
        } else if !title.is_empty() {
            addresses.add_other(title);
        }
        if let Some(name) = subagent.filter(|name| !name.is_empty()) {
            addresses.add_other(name);
        }
        addresses.add_other(id);
    }
    Ok(addresses)
}

/// A reachable Session by exact ID: a root Session or a child of the caller, archived or
/// not (the send pipeline refuses archived targets with a record).
fn by_id(db: &Connection, caller: Option<&str>, id: &str) -> rusqlite::Result<Option<Target>> {
    db.query_row(
        &format!(
            "SELECT {COLUMNS} FROM session WHERE id = ?1 AND (parent_id IS NULL OR parent_id IS ?2)"
        ),
        params![id, caller],
        |row| target(row, caller),
    )
    .optional()
}

/// The name tiers, highest first; every match is shown by its name.
fn name_tiers(
    db: &Connection,
    caller: Option<&str>,
    name: &str,
) -> rusqlite::Result<Vec<Vec<Target>>> {
    let named = |mut targets: Vec<Target>| {
        for t in &mut targets {
            t.address = name.to_owned();
        }
        targets
    };
    let subagents = match caller {
        Some(caller) => query(
            db,
            Some(caller),
            "parent_id = ?2 AND subagent_name = ?1 AND archived = 0",
            params![name, caller],
        )?,
        None => Vec::new(),
    };
    let roots = query(
        db,
        caller,
        "parent_id IS NULL AND title = ?1 AND archived = 0",
        params![name],
    )?;
    let team = match caller {
        Some(caller) => team_tier(db, caller, name)?,
        None => Vec::new(),
    };
    Ok(vec![named(team), named(subagents), named(roots)])
}

/// The address that names every other non-stopped member of the caller's team.
pub const ALL: &str = "all";

/// The members of `caller`'s team that `name` names (`agent-teams` → Teammate
/// messaging): the member called `name` (`lead` is the lead), or for [`ALL`] every
/// other member that is not `stopped`, lead first, then teammates in spawn order. Each
/// is a teammate shown by its member name. Empty for a caller in no team.
fn team_tier(db: &Connection, caller: &str, name: &str) -> rusqlite::Result<Vec<Target>> {
    let mut stmt = db.prepare(
        "SELECT s.id, s.title, s.directory, s.parent_id, s.subagent_name, s.archived,
                s.inbound, s.inbound_reason, m.name
           FROM team_member c
           JOIN team_member m ON m.team_id = c.team_id AND m.session_id <> c.session_id
           JOIN session s ON s.id = m.session_id
          WHERE c.session_id = ?1
            AND (m.name = ?2 OR (?2 = ?3 AND m.status <> 'stopped'))
          ORDER BY m.role = 'teammate', m.rowid",
    )?;
    stmt.query_map(params![caller, name, ALL], |row| {
        let mut t = target(row, Some(caller))?;
        t.candidate.kind = AddressKind::Teammate;
        t.candidate.name = row.get(NEXT_COLUMN)?;
        Ok(t)
    })?
    .collect()
}

/// The targets of a send from `caller` to `all`, each addressed by its member name;
/// `None` when the caller is in no team (then `all` is an ordinary name).
pub fn team_all(db: &Connection, caller: &str) -> rusqlite::Result<Option<Vec<Target>>> {
    if team_name(db, caller)?.is_none() {
        return Ok(None);
    }
    let mut targets = team_tier(db, caller, ALL)?;
    for t in &mut targets {
        t.address = t.candidate.name.clone();
    }
    Ok(Some(targets))
}

/// The member name of `session` in its team.
fn team_name(db: &Connection, session: &str) -> rusqlite::Result<Option<String>> {
    db.query_row(
        "SELECT name FROM team_member WHERE session_id = ?1",
        [session],
        |row| row.get(0),
    )
    .optional()
}

/// The member name `a` knows `b` by when both are in one team.
pub fn teammate_name(db: &Connection, a: &str, b: &str) -> rusqlite::Result<Option<String>> {
    db.query_row(
        "SELECT m.name FROM team_member m JOIN team_member c ON c.team_id = m.team_id
          WHERE c.session_id = ?1 AND m.session_id = ?2 AND m.session_id <> c.session_id",
        params![a, b],
        |row| row.get(0),
    )
    .optional()
}

fn query(
    db: &Connection,
    caller: Option<&str>,
    filter: &str,
    args: impl rusqlite::Params,
) -> rusqlite::Result<Vec<Target>> {
    let mut stmt = db.prepare(&format!(
        "SELECT {COLUMNS} FROM session WHERE {filter} ORDER BY created_at, id"
    ))?;
    stmt.query_map(args, |row| target(row, caller))?.collect()
}

/// The address `target` replies to `sender` with: the sender's member name when both are
/// in one team, else its name when that name resolves to the sender from `target` (a
/// subagent's name from its parent, a unique title otherwise), else its ID.
pub fn reply_address(db: &Connection, target: &str, sender: &str) -> rusqlite::Result<String> {
    if let Some(name) = teammate_name(db, target, sender)? {
        return Ok(name);
    }
    let row = db
        .query_row(
            &format!("SELECT {COLUMNS} FROM session WHERE id = ?1"),
            [sender],
            |row| self::target(row, Some(target)),
        )
        .optional()?;
    let Some(name) = row.map(|t| t.candidate.name) else {
        return Ok(sender.to_owned());
    };
    let resolves = addressable_name(&name)
        && matches!(resolve(db, Some(target), &name)?, Ok(t) if t.candidate.session_id == sender);
    Ok(if resolves { name } else { sender.to_owned() })
}

/// The Sessions `caller` can message, newest first, at most [`REACHABLE_LIMIT`]: its
/// subagents (archived ones too, which cannot receive) and every other non-archived root
/// Session, optionally in one `directory`. Each is addressed by its name when that name
/// resolves to it from the caller, else by its ID.
pub fn reachable(
    db: &Connection,
    caller: Option<&str>,
    directory: Option<&str>,
) -> rusqlite::Result<Vec<Target>> {
    let mut stmt = db.prepare(&format!(
        "SELECT {COLUMNS},
            (SELECT count(*) FROM session o
              WHERE o.parent_id IS NULL AND o.archived = 0 AND o.title = s.title),
            (SELECT count(*) FROM session c
              WHERE c.parent_id = ?1 AND c.archived = 0
                AND c.subagent_name = coalesce(s.subagent_name, s.title))
         FROM session s
         WHERE ((?1 IS NOT NULL AND s.parent_id = ?1)
                OR (s.parent_id IS NULL AND s.archived = 0 AND s.id IS NOT ?1))
           AND (?2 IS NULL OR s.directory = ?2)
         ORDER BY s.updated_at DESC, s.rowid DESC LIMIT ?3"
    ))?;
    stmt.query_map(params![caller, directory, REACHABLE_LIMIT], |row| {
        let mut t = target(row, caller)?;
        let (roots, subagents): (i64, i64) = (row.get(NEXT_COLUMN)?, row.get(NEXT_COLUMN + 1)?);
        let unique = match t.candidate.kind {
            AddressKind::Subagent => {
                !t.archived && subagents == 1 && row.get::<_, Option<String>>(4)?.is_some()
            }
            _ => roots == 1 && subagents == 0,
        };
        if unique && addressable_name(&t.candidate.name) {
            t.address = t.candidate.name.clone();
        }
        Ok(t)
    })?
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(id: &str) -> Target {
        Target {
            address: id.into(),
            candidate: Candidate {
                session_id: id.into(),
                name: "migration".into(),
                kind: AddressKind::Local,
                location: SessionLocation {
                    directory: "/repo".into(),
                    workspace: None,
                },
            },
            archived: false,
            inbound: None,
            peer: None,
        }
    }

    /// A team `team_1` of lead `ses_lead`, `api`, `ui` and the stopped `docs`, and
    /// `ses_out` in no team, which has a root titled `all`.
    fn team_store() -> cyber_store::Store {
        let store = cyber_store::Store::open(cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::Memory,
            crate::runtime::Runtime::registry(),
        ))
        .unwrap();
        store
            .transaction(|tx| {
                tx.execute_batch(
                    "INSERT INTO session (id, title, directory, agent, model, mode, created_at, updated_at, last_seq)
                     VALUES ('ses_lead', 'planner', '/r', 'build', 'test/main', 'default', 1, 1, -1),
                            ('ses_api', 'adhoc/api', '/r', 'build', 'test/main', 'default', 2, 2, -1),
                            ('ses_ui', 'adhoc/ui', '/r', 'build', 'test/main', 'default', 3, 3, -1),
                            ('ses_docs', 'adhoc/docs', '/r', 'build', 'test/main', 'default', 4, 4, -1),
                            ('ses_out', 'all', '/r', 'build', 'test/main', 'default', 5, 5, -1);
                     INSERT INTO team (id, name, lead_id, directory, status, created_ms, updated_ms)
                     VALUES ('team_1', 'adhoc', 'ses_lead', '/r', 'active', 1, 1);
                     INSERT INTO team_member (team_id, name, session_id, role, agent, isolation, status)
                     VALUES ('team_1', 'lead', 'ses_lead', 'lead', 'build', 'shared', 'idle'),
                            ('team_1', 'api', 'ses_api', 'teammate', 'build', 'shared', 'working'),
                            ('team_1', 'ui', 'ses_ui', 'teammate', 'build', 'shared', 'idle'),
                            ('team_1', 'docs', 'ses_docs', 'teammate', 'build', 'shared', 'stopped');",
                )?;
                Ok(())
            })
            .unwrap();
        store
    }

    #[test]
    fn team_members_resolve_by_member_name_and_all_skips_the_stopped() {
        let store = team_store();
        store
            .read(|db| {
                let resolved = |caller: &str, to: &str| {
                    resolve(db, Some(caller), to).unwrap().map(|t| {
                        (
                            t.address,
                            t.candidate.session_id,
                            t.candidate.name,
                            t.candidate.kind,
                        )
                    })
                };
                let teammate = |address: &str, id: &str, name: &str| {
                    Ok((
                        address.into(),
                        id.into(),
                        name.into(),
                        AddressKind::Teammate,
                    ))
                };
                assert_eq!(
                    resolved("ses_ui", "lead"),
                    teammate("lead", "ses_lead", "lead")
                );
                assert_eq!(
                    resolved("ses_ui", "@api"),
                    teammate("api", "ses_api", "api")
                );
                // A stopped member is still addressed by its name.
                assert_eq!(
                    resolved("ses_lead", "docs"),
                    teammate("docs", "ses_docs", "docs")
                );
                // A member's own name is not a teammate of it.
                assert!(matches!(
                    resolved("ses_ui", "ui"),
                    Err(AddressError::NotFound(_))
                ));
                // Outside the team the names mean nothing, and `all` is an ordinary title.
                assert!(matches!(
                    resolved("ses_out", "lead"),
                    Err(AddressError::NotFound(_))
                ));
                assert_eq!(team_all(db, "ses_out").unwrap(), None);
                // `all` fans out to every other member that is not stopped, lead first.
                let all = team_all(db, "ses_ui").unwrap().unwrap();
                let named: Vec<(&str, &str)> = all
                    .iter()
                    .map(|t| (t.address.as_str(), t.candidate.session_id.as_str()))
                    .collect();
                assert_eq!(named, [("lead", "ses_lead"), ("api", "ses_api")]);
                let all = team_all(db, "ses_lead").unwrap().unwrap();
                let names: Vec<&str> = all.iter().map(|t| t.address.as_str()).collect();
                assert_eq!(names, ["api", "ui"]);
                // Replies name a teammate by its member name.
                assert_eq!(reply_address(db, "ses_lead", "ses_ui").unwrap(), "ui");
                assert_eq!(reply_address(db, "ses_ui", "ses_lead").unwrap(), "lead");
                assert_eq!(reply_address(db, "ses_out", "ses_ui").unwrap(), "adhoc/ui");
                assert_eq!(teammate_name(db, "ses_ui", "ses_ui").unwrap(), None);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_leading_at_is_stripped_once_and_any_other_at_is_remote() {
        assert_eq!(parse("@migration"), Ok(Address::Name("migration".into())));
        assert_eq!(parse("ses_1"), Ok(Address::Id("ses_1".into())));
        let remote = |session: &str, peer: &str| {
            Ok(Address::Remote {
                session: session.into(),
                peer: peer.into(),
            })
        };
        assert_eq!(parse("api@laptop"), remote("api", "laptop"));
        assert_eq!(parse("ses_1@laptop"), remote("ses_1", "laptop"));
        assert_eq!(parse("@api@laptop"), remote("api", "laptop"));
        assert_eq!(
            parse("review notes@desk.lan"),
            remote("review notes", "desk.lan")
        );
        // No relaying through a peer, no empty parts and no peer name `@` cannot carry.
        for bad in ["@@migration", "a@b@c", "api@", "api@-x", "api@a b"] {
            assert!(
                matches!(parse(bad), Err(AddressError::NotFound(_))),
                "{bad}"
            );
        }
        assert_eq!(parse("@"), Err(AddressError::NotFound("@".into())));
        assert!(addressable_name("migration"));
        for name in ["@migration", "a@b", "ses_x", ""] {
            assert!(!addressable_name(name), "{name}");
        }
    }

    #[test]
    fn only_plain_names_are_trusted_titles() {
        assert!(trusted_title("migration"));
        assert!(trusted_title("client-api"));
        for title in [
            "",
            "client",
            "client:cli",
            "mcp:zed",
            "ses_01JABC",
            "@migration",
            "api@laptop",
        ] {
            assert!(!trusted_title(title), "{title}");
        }
    }

    #[test]
    fn the_first_matching_tier_decides_and_several_matches_are_ambiguous() {
        let shadowing = pick(
            "api",
            vec![vec![], vec![local("ses_sub")], vec![local("ses_root")]],
        );
        assert_eq!(shadowing.unwrap().candidate.session_id, "ses_sub");
        let ambiguous = pick(
            "migration",
            vec![vec![], vec![local("ses_a"), local("ses_b")]],
        );
        let Err(AddressError::Ambiguous { candidates, .. }) = ambiguous else {
            panic!("{ambiguous:?}");
        };
        let ids: Vec<_> = candidates.iter().map(|c| c.session_id.as_str()).collect();
        assert_eq!(ids, ["ses_a", "ses_b"]);
        assert_eq!(
            pick("ghost", vec![vec![], vec![]]),
            Err(AddressError::NotFound("ghost".into()))
        );
    }
}
