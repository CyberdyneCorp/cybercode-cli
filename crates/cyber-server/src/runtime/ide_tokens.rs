//! Local IDE tokens (`server-api` → Local IDE token issuance, Local IDE token revocation
//! and expiry): a scoped, revocable local credential for editor clients.
//!
//! A token is `cyi_` followed by 32 random bytes as unpadded base64url (43 characters).
//! Only the hex SHA-256 of the whole token is stored, under a unique index, so a check is
//! one indexed lookup on the reader connection and the database never holds a usable
//! token. A malformed value is refused before the database is touched. Only a valid token
//! writes, to record `last_used_ms` at most once per [`LAST_USED_GRANULARITY`]. Each
//! client has at most one unrevoked token: issuing another revokes the previous one in the
//! same transaction. Revoked and expired records stay listable for [`RETENTION_MS`].

use base64::Engine;
use rusqlite::{OptionalExtension, Row, params};
use sha2::{Digest, Sha256};

use cyber_store::{Store, StoreError};

/// The prefix of every IDE token (also a redaction pattern).
pub const TOKEN_PREFIX: &str = "cyi_";
/// Clients that may hold a token.
pub const CLIENTS: &[&str] = &["vscode"];
/// Default lifetime in days.
pub const DEFAULT_DAYS: u32 = 30;
/// Longest lifetime in days.
pub const MAX_DAYS: u32 = 90;
/// `last_used_ms` is rewritten only when it is at least this old (milliseconds).
pub const LAST_USED_GRANULARITY: i64 = 60_000;
/// Revoked and expired records are deleted this long after the fact (30 days).
pub const RETENTION_MS: i64 = 30 * DAY_MS;
const DAY_MS: i64 = 86_400_000;
const SECRET_BYTES: usize = 32;
const SECRET_CHARS: usize = 43;
const MAX_NAME_CHARS: usize = 64;

/// A token record as stored, without its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeToken {
    /// `itk_<ulid>`.
    pub id: String,
    pub client: String,
    pub name: Option<String>,
    pub created_ms: i64,
    pub expires_ms: i64,
    pub last_used_ms: Option<i64>,
    pub revoked_ms: Option<i64>,
}

impl IdeToken {
    /// Neither revoked nor past its expiry at `now_ms`.
    pub fn active(&self, now_ms: i64) -> bool {
        self.revoked_ms.is_none() && now_ms < self.expires_ms
    }
}

/// What `cyber ide token create` asks for.
#[derive(Debug, Clone, Default)]
pub struct NewToken {
    pub client: String,
    pub name: Option<String>,
    /// `None` means [`DEFAULT_DAYS`].
    pub expires_in_days: Option<u32>,
}

/// A created token and its value, which exists only here.
#[derive(Clone)]
pub struct Issued {
    pub record: IdeToken,
    pub token: String,
    /// The token this one replaced, now revoked.
    pub replaced: Option<String>,
}

impl std::fmt::Debug for Issued {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Issued")
            .field("record", &self.record)
            .field("token", &"<redacted>")
            .field("replaced", &self.replaced)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IdeTokenError {
    #[error("unknown IDE client {0:?}: use vscode")]
    UnknownClient(String),
    #[error("expires_in_days must be between 1 and {MAX_DAYS}")]
    InvalidExpiry,
    #[error("the name must be at most {MAX_NAME_CHARS} printable characters")]
    InvalidName,
    #[error("cannot generate a token: {0}")]
    Random(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Issue a token for `request.client`, revoking that client's previous token in the same
/// transaction, and delete records past their retention.
pub fn create(store: &Store, request: NewToken, now_ms: i64) -> Result<Issued, IdeTokenError> {
    if !CLIENTS.contains(&request.client.as_str()) {
        return Err(IdeTokenError::UnknownClient(request.client));
    }
    let days = request.expires_in_days.unwrap_or(DEFAULT_DAYS);
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(IdeTokenError::InvalidExpiry);
    }
    let name = request.name.filter(|name| !name.trim().is_empty());
    if name.as_deref().is_some_and(|name| {
        name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control)
    }) {
        return Err(IdeTokenError::InvalidName);
    }
    let mut secret = [0u8; SECRET_BYTES];
    getrandom::fill(&mut secret).map_err(|error| IdeTokenError::Random(error.to_string()))?;
    let token = format!(
        "{TOKEN_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
    );
    let record = IdeToken {
        id: cyber_core::ids::new_id("itk"),
        client: request.client,
        name,
        created_ms: now_ms,
        expires_ms: now_ms + i64::from(days) * DAY_MS,
        last_used_ms: None,
        revoked_ms: None,
    };
    let row = record.clone();
    let hash = sha256_hex(&token);
    let replaced = store.transaction(move |tx| {
        prune(tx, now_ms)?;
        let replaced: Option<String> = tx
            .query_row(
                "SELECT id FROM ide_token WHERE client = ?1 AND revoked_ms IS NULL",
                [&row.client],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute(
            "UPDATE ide_token SET revoked_ms = ?2 WHERE client = ?1 AND revoked_ms IS NULL",
            params![row.client, now_ms],
        )?;
        tx.execute(
            "INSERT INTO ide_token (id, client, name, sha256, created_ms, expires_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row.id,
                row.client,
                row.name,
                hash,
                row.created_ms,
                row.expires_ms
            ],
        )?;
        Ok(replaced)
    })?;
    Ok(Issued {
        record,
        token,
        replaced,
    })
}

/// Whether `value` has the shape of an IDE token: the prefix and 43 base64url characters.
pub fn well_formed(value: &str) -> bool {
    value.strip_prefix(TOKEN_PREFIX).is_some_and(|secret| {
        secret.len() == SECRET_CHARS
            && secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    })
}

/// The active token record `token` belongs to, recording `now_ms` as its last use when
/// the recorded one is older than [`LAST_USED_GRANULARITY`]; `None` for a malformed,
/// unknown, revoked or expired token. A malformed token never reaches the database.
pub fn verify(store: &Store, token: &str, now_ms: i64) -> Result<Option<IdeToken>, StoreError> {
    if !well_formed(token) {
        return Ok(None);
    }
    let presented = sha256_hex(token);
    let lookup = presented.clone();
    let found = store.read(move |conn| {
        Ok(conn
            .query_row(
                &format!("SELECT {COLUMNS}, sha256 FROM ide_token WHERE sha256 = ?1"),
                [&lookup],
                |row| Ok((token_row(row)?, row.get::<_, String>(7)?)),
            )
            .optional()?)
    })?;
    let Some((mut record, stored)) = found else {
        return Ok(None);
    };
    if !constant_eq(stored.as_bytes(), presented.as_bytes()) || !record.active(now_ms) {
        return Ok(None);
    }
    let recent = record
        .last_used_ms
        .is_some_and(|used| now_ms.saturating_sub(used) < LAST_USED_GRANULARITY);
    if recent {
        return Ok(Some(record));
    }
    // Still active when the use is recorded: a revocation in between wins.
    let id = record.id.clone();
    let recorded = store.transaction(move |tx| {
        let changed = tx.execute(
            "UPDATE ide_token SET last_used_ms = ?2 WHERE id = ?1 AND revoked_ms IS NULL \
             AND expires_ms > ?2",
            params![id, now_ms],
        )?;
        Ok(changed > 0)
    })?;
    if !recorded {
        return Ok(None);
    }
    record.last_used_ms = Some(now_ms);
    Ok(Some(record))
}

/// Revoke token `id` from the next request on. Idempotent: an already revoked token keeps
/// its first revocation time. `None` when no record has that ID.
pub fn revoke(store: &Store, id: &str, now_ms: i64) -> Result<Option<IdeToken>, StoreError> {
    let id = id.to_string();
    store.transaction(move |tx| {
        tx.execute(
            "UPDATE ide_token SET revoked_ms = ?2 WHERE id = ?1 AND revoked_ms IS NULL",
            params![id, now_ms],
        )?;
        Ok(tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM ide_token WHERE id = ?1"),
                [&id],
                token_row,
            )
            .optional()?)
    })
}

/// Revoke every unrevoked token (the owner password was replaced); returns their IDs.
pub fn revoke_all(store: &Store, now_ms: i64) -> Result<Vec<String>, StoreError> {
    store.transaction(move |tx| {
        let ids = {
            let mut stmt = tx.prepare("SELECT id FROM ide_token WHERE revoked_ms IS NULL")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        tx.execute(
            "UPDATE ide_token SET revoked_ms = ?1 WHERE revoked_ms IS NULL",
            [now_ms],
        )?;
        Ok(ids)
    })
}

/// Every record still within retention, oldest first, revoked and expired ones included.
/// Records past their retention are deleted first.
pub fn list(store: &Store, now_ms: i64) -> Result<Vec<IdeToken>, StoreError> {
    store.transaction(move |tx| {
        prune(tx, now_ms)?;
        let mut stmt = tx.prepare(&format!(
            "SELECT {COLUMNS} FROM ide_token ORDER BY created_ms, id"
        ))?;
        let rows = stmt.query_map([], token_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
}

/// Delete records revoked or expired more than [`RETENTION_MS`] before `now_ms`.
fn prune(tx: &rusqlite::Transaction<'_>, now_ms: i64) -> rusqlite::Result<usize> {
    let cutoff = now_ms - RETENTION_MS;
    tx.execute(
        "DELETE FROM ide_token WHERE (revoked_ms IS NOT NULL AND revoked_ms < ?1) \
         OR expires_ms < ?1",
        [cutoff],
    )
}

const COLUMNS: &str = "id, client, name, created_ms, expires_ms, last_used_ms, revoked_ms";

fn token_row(row: &Row<'_>) -> rusqlite::Result<IdeToken> {
    Ok(IdeToken {
        id: row.get(0)?,
        client: row.get(1)?,
        name: row.get(2)?,
        created_ms: row.get(3)?,
        expires_ms: row.get(4)?,
        last_used_ms: row.get(5)?,
        revoked_ms: row.get(6)?,
    })
}

fn sha256_hex(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
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

    fn issue(store: &Store, now: i64) -> Issued {
        let request = NewToken {
            client: "vscode".into(),
            ..NewToken::default()
        };
        create(store, request, now).unwrap()
    }

    fn hashes(store: &Store) -> Vec<String> {
        store
            .read(|conn| {
                let mut stmt = conn.prepare("SELECT sha256 FROM ide_token")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap()
    }

    #[test]
    fn a_token_is_random_base64url_and_only_its_hash_is_stored() {
        let store = store();
        let issued = issue(&store, 1_000);
        assert!(well_formed(&issued.token), "{}", issued.token.len());
        assert!(issued.record.id.starts_with("itk_"));
        assert_eq!(issued.record.expires_ms, 1_000 + 30 * DAY_MS);
        assert_eq!(hashes(&store), vec![sha256_hex(&issued.token)]);
        assert!(!format!("{issued:?}").contains(&issued.token));
        let found = verify(&store, &issued.token, 2_000).unwrap().unwrap();
        assert_eq!(found.id, issued.record.id);
        assert_eq!(found.last_used_ms, Some(2_000));
    }

    #[test]
    fn unknown_clients_names_and_expiries_are_refused() {
        let store = store();
        let refused = |request: NewToken| create(&store, request, 0).unwrap_err();
        assert!(matches!(
            refused(NewToken {
                client: "jetbrains".into(),
                ..NewToken::default()
            }),
            IdeTokenError::UnknownClient(_)
        ));
        for days in [0, 91] {
            assert!(matches!(
                refused(NewToken {
                    client: "vscode".into(),
                    expires_in_days: Some(days),
                    ..NewToken::default()
                }),
                IdeTokenError::InvalidExpiry
            ));
        }
        assert!(matches!(
            refused(NewToken {
                client: "vscode".into(),
                name: Some("a\nb".into()),
                ..NewToken::default()
            }),
            IdeTokenError::InvalidName
        ));
        for days in [1, 90] {
            let issued = create(
                &store,
                NewToken {
                    client: "vscode".into(),
                    expires_in_days: Some(days),
                    ..NewToken::default()
                },
                0,
            )
            .unwrap();
            assert_eq!(issued.record.expires_ms, i64::from(days) * DAY_MS);
        }
    }

    #[test]
    fn reissuing_rotates_the_previous_token() {
        let store = store();
        let first = issue(&store, 1_000);
        let second = issue(&store, 2_000);
        assert_eq!(second.replaced.as_deref(), Some(first.record.id.as_str()));
        assert!(verify(&store, &first.token, 3_000).unwrap().is_none());
        assert!(verify(&store, &second.token, 3_000).unwrap().is_some());
        let listed = list(&store, 3_000).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].revoked_ms, Some(2_000));
        assert_eq!(listed[1].revoked_ms, None);
    }

    #[test]
    fn malformed_unknown_expired_and_revoked_tokens_are_refused() {
        let store = store();
        let issued = issue(&store, 0);
        let unknown = format!("{TOKEN_PREFIX}{}", "A".repeat(SECRET_CHARS));
        for value in [
            "",
            "cyi_",
            "cyi_short",
            "Bearer cyi_",
            unknown.as_str(),
            &issued.token[..issued.token.len() - 1],
        ] {
            assert!(verify(&store, value, 1).unwrap().is_none(), "{value}");
        }
        assert!(!well_formed(&format!("{TOKEN_PREFIX}{}", "+".repeat(43))));
        // Expiry: a token is refused from its expiry on.
        assert!(
            verify(&store, &issued.token, 30 * DAY_MS - 1)
                .unwrap()
                .is_some()
        );
        assert!(
            verify(&store, &issued.token, 30 * DAY_MS)
                .unwrap()
                .is_none()
        );
        let other = issue(&store, 10);
        let revoked = revoke(&store, &other.record.id, 20).unwrap().unwrap();
        assert_eq!(revoked.revoked_ms, Some(20));
        assert!(verify(&store, &other.token, 21).unwrap().is_none());
    }

    #[test]
    fn revocation_is_idempotent() {
        let store = store();
        let issued = issue(&store, 0);
        let first = revoke(&store, &issued.record.id, 5).unwrap().unwrap();
        let again = revoke(&store, &issued.record.id, 9).unwrap().unwrap();
        assert_eq!(first.revoked_ms, Some(5));
        assert_eq!(again.revoked_ms, Some(5));
        assert!(revoke(&store, "itk_missing", 9).unwrap().is_none());
    }

    #[test]
    fn last_use_is_recorded_at_most_once_a_minute() {
        let store = store();
        let issued = issue(&store, 0);
        let used = |now| verify(&store, &issued.token, now).unwrap().unwrap();
        assert_eq!(used(1_000).last_used_ms, Some(1_000));
        assert_eq!(used(30_000).last_used_ms, Some(1_000));
        assert_eq!(used(61_000).last_used_ms, Some(61_000));
    }

    #[test]
    fn revoke_all_revokes_every_active_token() {
        let store = store();
        let issued = issue(&store, 0);
        assert_eq!(revoke_all(&store, 7).unwrap(), vec![issued.record.id]);
        assert!(verify(&store, &issued.token, 8).unwrap().is_none());
        assert!(revoke_all(&store, 9).unwrap().is_empty());
    }

    #[test]
    fn retention_deletes_only_records_older_than_thirty_days() {
        let store = store();
        let revoked = issue(&store, 0);
        revoke(&store, &revoked.record.id, 1_000).unwrap();
        let expiring = create(
            &store,
            NewToken {
                client: "vscode".into(),
                expires_in_days: Some(1),
                ..NewToken::default()
            },
            2_000,
        )
        .unwrap();
        let expired_at = expiring.record.expires_ms;
        let ids = |now| -> Vec<String> {
            list(&store, now)
                .unwrap()
                .into_iter()
                .map(|t| t.id)
                .collect()
        };
        assert_eq!(
            ids(1_000 + RETENTION_MS).len(),
            2,
            "exactly 30 days is kept"
        );
        assert_eq!(ids(1_001 + RETENTION_MS), vec![expiring.record.id.clone()]);
        assert_eq!(
            ids(expired_at + RETENTION_MS),
            vec![expiring.record.id.clone()]
        );
        assert!(ids(expired_at + RETENTION_MS + 1).is_empty());
    }

    #[test]
    fn tokens_survive_reopening_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let open = || {
            Store::open(cyber_store::StoreOptions::new(
                cyber_core::paths::DatabaseLocation::File(dir.path().join("cyber.db")),
                crate::runtime::Runtime::registry(),
            ))
            .unwrap()
        };
        let issued = issue(&open(), 0);
        let reopened = open();
        assert!(verify(&reopened, &issued.token, 1).unwrap().is_some());
    }
}
