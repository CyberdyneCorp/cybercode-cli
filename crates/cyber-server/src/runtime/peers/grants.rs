//! Peer grants (`cross-session-messaging` → Peer pairing and grants): who may call this
//! server's peer listener, with which scopes, until when.
//!
//! A token is `cyp_pgr<ulid>_<secret>`: the grant's ID without its underscore, then 32
//! random bytes as unpadded base64url (43 characters). Only the hex SHA-256 of the secret
//! is stored, so the database never holds a usable token; SHA-256 is enough for a 256-bit
//! random secret, and the comparison is constant-time. A check reads the row on the reader
//! connection, so a request with an unknown or wrong token never takes the writer lock the
//! Sessions commit through; only a valid token writes, to record `last_used_ms` (at most
//! once per [`LAST_USED_GRANULARITY`]), and that write re-checks the grant is still active.
//! A revocation or an expiry applies to the next request.

use base64::Engine;
use rusqlite::{OptionalExtension, Row, params};
use sha2::{Digest, Sha256};

use cyber_store::{Store, StoreError};

use crate::runtime::messaging::PeerScope;

/// The prefix of every peer token (also a redaction pattern).
pub const TOKEN_PREFIX: &str = "cyp_";
/// The prefix of a grant ID.
const ID_PREFIX: &str = "pgr_";
/// Random bytes in a token's secret.
const SECRET_BYTES: usize = 32;
/// Length of the secret as unpadded base64url.
const SECRET_CHARS: usize = 43;
/// `last_used_ms` is rewritten only when it is at least this old (milliseconds).
pub const LAST_USED_GRANULARITY: i64 = 60_000;

/// A grant as stored, without its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// `pgr_<ulid>`.
    pub id: String,
    /// The peer's name on this machine.
    pub name: String,
    /// Sorted, without duplicates; never empty.
    pub scopes: Vec<PeerScope>,
    pub created_ms: i64,
    pub expires_ms: Option<i64>,
    pub last_used_ms: Option<i64>,
    pub revoked_ms: Option<i64>,
}

impl Grant {
    pub fn has(&self, scope: PeerScope) -> bool {
        self.scopes.contains(&scope)
    }

    /// Neither revoked nor past its expiry at `now_ms`.
    pub fn active(&self, now_ms: i64) -> bool {
        self.revoked_ms.is_none() && self.expires_ms.is_none_or(|expires| now_ms < expires)
    }
}

/// What `cyber peers invite` asks for.
#[derive(Debug, Clone)]
pub struct NewGrant {
    pub name: String,
    /// Empty means the default, `message`.
    pub scopes: Vec<PeerScope>,
    pub expires_ms: Option<i64>,
}

/// A created grant and its token, which exists only here.
#[derive(Clone)]
pub struct Issued {
    pub grant: Grant,
    pub token: String,
}

impl std::fmt::Debug for Issued {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Issued")
            .field("grant", &self.grant)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GrantError {
    #[error("invalid peer name {0:?}: use letters, digits, '.', '_' or '-' (at most 64)")]
    InvalidName(String),
    #[error("a grant named `{0}` is active; revoke it first with `cyber peers revoke {0}`")]
    NameInUse(String),
    #[error("the expiry must be in the future")]
    ExpiryInPast,
    #[error("cannot generate a token: {0}")]
    Random(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Create a grant and its token. A revoked or expired grant of the same name is replaced;
/// an active one is not.
pub fn create(store: &Store, request: NewGrant, now_ms: i64) -> Result<Issued, GrantError> {
    if !cyber_core::config::valid_peer_name(&request.name) {
        return Err(GrantError::InvalidName(request.name));
    }
    if request.expires_ms.is_some_and(|expires| expires <= now_ms) {
        return Err(GrantError::ExpiryInPast);
    }
    let mut scopes = request.scopes;
    if scopes.is_empty() {
        scopes.push(PeerScope::Message);
    }
    scopes.sort_unstable();
    scopes.dedup();
    let mut secret = [0u8; SECRET_BYTES];
    getrandom::fill(&mut secret).map_err(|error| GrantError::Random(error.to_string()))?;
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret);
    let id = cyber_core::ids::new_id("pgr");
    let token = format!("{TOKEN_PREFIX}{}_{secret}", id.replacen('_', "", 1));
    let grant = Grant {
        id,
        name: request.name,
        scopes,
        created_ms: now_ms,
        expires_ms: request.expires_ms,
        last_used_ms: None,
        revoked_ms: None,
    };
    let row = grant.clone();
    let hash = sha256_hex(&secret);
    store.transaction(move |tx| {
        let existing = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM peer_grant WHERE name = ?1"),
                [&row.name],
                grant_row,
            )
            .optional()?;
        if existing.is_some_and(|existing| existing.active(now_ms)) {
            return Ok(Err(GrantError::NameInUse(row.name)));
        }
        tx.execute("DELETE FROM peer_grant WHERE name = ?1", [&row.name])?;
        tx.execute(
            "INSERT INTO peer_grant (id, name, token_sha256, scopes, created_ms, expires_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row.id,
                row.name,
                hash,
                scopes_json(&row.scopes),
                row.created_ms,
                row.expires_ms
            ],
        )?;
        Ok(Ok(()))
    })??;
    Ok(Issued { grant, token })
}

/// The active grant `token` belongs to, recording `now_ms` as its last use when the
/// recorded one is older than [`LAST_USED_GRANULARITY`]; `None` for a malformed, unknown,
/// revoked or expired token.
pub fn verify(store: &Store, token: &str, now_ms: i64) -> Result<Option<Grant>, StoreError> {
    let Some((id, secret)) = parse(token) else {
        return Ok(None);
    };
    let presented = sha256_hex(secret);
    let lookup = id.clone();
    let found = store.read(move |conn| {
        Ok(conn
            .query_row(
                &format!("SELECT {COLUMNS}, token_sha256 FROM peer_grant WHERE id = ?1"),
                [&lookup],
                |row| Ok((grant_row(row)?, row.get::<_, String>(7)?)),
            )
            .optional()?)
    })?;
    let Some((mut grant, stored)) = found else {
        return Ok(None);
    };
    if !constant_eq(stored.as_bytes(), presented.as_bytes()) || !grant.active(now_ms) {
        return Ok(None);
    }
    let recent = grant
        .last_used_ms
        .is_some_and(|used| now_ms.saturating_sub(used) < LAST_USED_GRANULARITY);
    if recent {
        return Ok(Some(grant));
    }
    // Still active when the use is recorded: a revocation in between wins.
    let recorded = store.transaction(move |tx| {
        let changed = tx.execute(
            "UPDATE peer_grant SET last_used_ms = ?2 WHERE id = ?1 AND revoked_ms IS NULL \
             AND (expires_ms IS NULL OR expires_ms > ?2)",
            params![id, now_ms],
        )?;
        Ok(changed > 0)
    })?;
    if !recorded {
        return Ok(None);
    }
    grant.last_used_ms = Some(now_ms);
    Ok(Some(grant))
}

/// Whether grant `id` exists and is active at `now_ms` (an open attach stream asks).
pub fn active(store: &Store, id: &str, now_ms: i64) -> Result<bool, StoreError> {
    let id = id.to_string();
    let found = store.read(move |conn| {
        Ok(conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM peer_grant WHERE id = ?1"),
                [&id],
                grant_row,
            )
            .optional()?)
    })?;
    Ok(found.is_some_and(|grant| grant.active(now_ms)))
}

/// Revoke the grant named `name` from the next request on; `false` when there is none or
/// it is already revoked.
pub fn revoke(store: &Store, name: &str, now_ms: i64) -> Result<bool, StoreError> {
    let name = name.to_string();
    store.transaction(move |tx| {
        let changed = tx.execute(
            "UPDATE peer_grant SET revoked_ms = ?2 WHERE name = ?1 AND revoked_ms IS NULL",
            params![name, now_ms],
        )?;
        Ok(changed > 0)
    })
}

/// Every grant, oldest first, revoked and expired ones included.
pub fn list(store: &Store) -> Result<Vec<Grant>, StoreError> {
    store.read(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM peer_grant ORDER BY created_ms, id"
        ))?;
        let rows = stmt.query_map([], grant_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
}

const COLUMNS: &str = "id, name, scopes, created_ms, expires_ms, last_used_ms, revoked_ms";

fn grant_row(row: &Row<'_>) -> rusqlite::Result<Grant> {
    let scopes: String = row.get(2)?;
    Ok(Grant {
        id: row.get(0)?,
        name: row.get(1)?,
        scopes: parse_scopes(&scopes),
        created_ms: row.get(3)?,
        expires_ms: row.get(4)?,
        last_used_ms: row.get(5)?,
        revoked_ms: row.get(6)?,
    })
}

/// Unknown scopes are dropped: a grant never gains a scope this version cannot name.
fn parse_scopes(json: &str) -> Vec<PeerScope> {
    serde_json::from_str::<Vec<String>>(json)
        .unwrap_or_default()
        .iter()
        .filter_map(|scope| PeerScope::parse(scope))
        .collect()
}

fn scopes_json(scopes: &[PeerScope]) -> String {
    serde_json::to_string(&scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .unwrap_or_else(|_| "[]".into())
}

/// `cyp_pgr<ulid>_<secret>` → (`pgr_<ulid>`, secret).
fn parse(token: &str) -> Option<(String, &str)> {
    let rest = token.strip_prefix(TOKEN_PREFIX)?.strip_prefix("pgr")?;
    let (ulid, secret) = rest.split_once('_')?;
    let well_formed = !ulid.is_empty()
        && ulid.bytes().all(|b| b.is_ascii_alphanumeric())
        && secret.len() == SECRET_CHARS
        && secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
    well_formed.then(|| (format!("{ID_PREFIX}{ulid}"), secret))
}

fn sha256_hex(secret: &str) -> String {
    format!("{:x}", Sha256::digest(secret.as_bytes()))
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let options = cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::Memory,
            crate::runtime::Runtime::registry(),
        );
        Store::open(options).unwrap()
    }

    fn invite(store: &Store, name: &str, scopes: Vec<PeerScope>) -> Issued {
        let request = NewGrant {
            name: name.into(),
            scopes,
            expires_ms: None,
        };
        create(store, request, 1_000).unwrap()
    }

    fn stored_hash(store: &Store, id: &str) -> String {
        let id = id.to_string();
        store
            .read(move |conn| {
                Ok(conn.query_row(
                    "SELECT token_sha256 FROM peer_grant WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    }

    #[test]
    fn a_token_names_its_grant_and_only_its_hash_is_stored() {
        let store = store();
        let issued = invite(&store, "laptop", vec![]);
        let token = &issued.token;
        assert!(token.starts_with("cyp_pgr"), "{token}");
        let secret = &token[token.len() - SECRET_CHARS..];
        assert_eq!(issued.grant.scopes, vec![PeerScope::Message]);
        assert_eq!(parse(token).unwrap().0, issued.grant.id);
        let hash = stored_hash(&store, &issued.grant.id);
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains(secret) && !hash.contains(token.as_str()));
        assert!(!format!("{issued:?}").contains(token.as_str()));
        assert!(cyber_core::memory::redact_credentials(token) != *token);

        let grant = verify(&store, token, 2_000).unwrap().expect("valid");
        assert_eq!(grant.name, "laptop");
        assert_eq!(grant.last_used_ms, Some(2_000));
        assert_eq!(list(&store).unwrap()[0].last_used_ms, Some(2_000));
    }

    #[test]
    fn wrong_malformed_and_foreign_tokens_are_refused() {
        let store = store();
        let issued = invite(&store, "laptop", vec![]);
        let other = invite(&store, "tablet", vec![]);
        // Another grant's secret under this grant's ID.
        let (prefix, _) = issued.token.split_at(issued.token.len() - SECRET_CHARS);
        let swapped = format!(
            "{prefix}{}",
            &other.token[other.token.len() - SECRET_CHARS..]
        );
        for token in [
            swapped.as_str(),
            "cyp_pgr01HZX_short",
            "cyp_",
            "Basic abc",
            &issued.token[..issued.token.len() - 1],
            &issued.token.replace("cyp_", "cyx_"),
        ] {
            assert_eq!(verify(&store, token, 2_000).unwrap(), None, "{token}");
        }
        assert_eq!(list(&store).unwrap()[0].last_used_ms, None);
    }

    #[test]
    fn revoked_and_expired_grants_stop_at_once() {
        let store = store();
        let issued = invite(
            &store,
            "laptop",
            vec![PeerScope::Attach, PeerScope::Message],
        );
        assert_eq!(
            issued.grant.scopes,
            vec![PeerScope::Message, PeerScope::Attach]
        );
        assert!(verify(&store, &issued.token, 2_000).unwrap().is_some());
        assert!(revoke(&store, "laptop", 3_000).unwrap());
        assert!(!revoke(&store, "laptop", 4_000).unwrap());
        assert_eq!(verify(&store, &issued.token, 3_000).unwrap(), None);
        assert_eq!(list(&store).unwrap()[0].revoked_ms, Some(3_000));

        let expiring = create(
            &store,
            NewGrant {
                name: "tablet".into(),
                scopes: vec![],
                expires_ms: Some(5_000),
            },
            1_000,
        )
        .unwrap();
        assert!(verify(&store, &expiring.token, 4_999).unwrap().is_some());
        assert_eq!(verify(&store, &expiring.token, 5_000).unwrap(), None);
    }

    /// Regression: every check, a forged token's included, took the writer lock the
    /// Sessions commit through, so unauthenticated requests could starve them. A check
    /// now reads on the reader connection and writes only for a valid token whose last
    /// use is older than a minute.
    #[test]
    fn checks_take_the_writer_only_to_record_a_valid_use() {
        let dir = tempfile::tempdir().unwrap();
        let options = cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::File(dir.path().join("cyber.db")),
            crate::runtime::Runtime::registry(),
        );
        let store = std::sync::Arc::new(Store::open(options).unwrap());
        let issued = invite(&store, "laptop", vec![]);
        assert!(verify(&store, &issued.token, 2_000).unwrap().is_some());

        // Hold the writer.
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let writer = std::sync::Arc::clone(&store);
        let holder = std::thread::spawn(move || {
            writer
                .transaction(move |_| {
                    held_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        });
        held_rx.recv().unwrap();
        let (prefix, _) = issued.token.split_at(issued.token.len() - SECRET_CHARS);
        let forged = format!("{prefix}{}", "A".repeat(SECRET_CHARS));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reader = std::sync::Arc::clone(&store);
        let token = issued.token.clone();
        std::thread::spawn(move || {
            let forged = verify(&reader, &forged, 3_000).unwrap();
            // Used a second ago: nothing to record.
            let recent = verify(&reader, &token, 3_000).unwrap();
            done_tx.send((forged, recent)).unwrap();
        });
        let checked = done_rx.recv_timeout(std::time::Duration::from_secs(10));
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        let (forged, recent) = checked.expect("a check waited for the writer");
        assert_eq!(forged, None);
        assert_eq!(recent.unwrap().last_used_ms, Some(2_000));

        // A minute later the use is recorded again; a revoked grant records nothing.
        let later = 2_000 + LAST_USED_GRANULARITY;
        let used = verify(&store, &issued.token, later).unwrap().unwrap();
        assert_eq!(used.last_used_ms, Some(later));
        assert!(active(&store, &issued.grant.id, later).unwrap());
        revoke(&store, "laptop", later + 1).unwrap();
        assert!(!active(&store, &issued.grant.id, later + 1).unwrap());
        assert!(!active(&store, "pgr_unknown", later).unwrap());
        assert_eq!(list(&store).unwrap()[0].last_used_ms, Some(later));
    }

    #[test]
    fn names_are_unique_while_active() {
        let store = store();
        let first = invite(&store, "laptop", vec![]);
        let again = NewGrant {
            name: "laptop".into(),
            scopes: vec![],
            expires_ms: None,
        };
        assert!(matches!(
            create(&store, again.clone(), 2_000),
            Err(GrantError::NameInUse(name)) if name == "laptop"
        ));
        revoke(&store, "laptop", 2_000).unwrap();
        let second = create(&store, again, 3_000).unwrap();
        assert_eq!(list(&store).unwrap().len(), 1);
        assert_eq!(verify(&store, &first.token, 4_000).unwrap(), None);
        assert!(verify(&store, &second.token, 4_000).unwrap().is_some());

        for name in ["", "a b", "@desk", "-x"] {
            let request = NewGrant {
                name: name.into(),
                scopes: vec![],
                expires_ms: None,
            };
            assert!(matches!(
                create(&store, request, 1_000),
                Err(GrantError::InvalidName(_))
            ));
        }
        let past = NewGrant {
            name: "old".into(),
            scopes: vec![],
            expires_ms: Some(1_000),
        };
        assert!(matches!(
            create(&store, past, 1_000),
            Err(GrantError::ExpiryInPast)
        ));
    }
}
