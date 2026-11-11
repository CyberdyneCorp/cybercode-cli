//! Immutable input-to-client bindings; live authority remains process-local.
use super::{RuntimeError, presence::InteractiveScope};
use cyber_store::{EventRegistry, NewEvent, Store, StoreError};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

pub(super) const BOUND: &str = "session.input.client_bound.1";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputClient {
    /// A live input-client lease, or null to explicitly request unattended input.
    pub client_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    message_id: String,
    client: InputClient,
    directory: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Attendance {
    Legacy,
    Unknown,
    Bound {
        session: String,
        directory: String,
        client: Option<String>,
    },
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256
}
pub(super) fn validate(client: &InputClient) -> Result<(), RuntimeError> {
    if client.client_id.as_deref().is_some_and(|id| !valid_id(id)) {
        return Err(RuntimeError::Invalid(
            "Invalid input-client identity".into(),
        ));
    }
    Ok(())
}
pub(super) fn validate_admission(client: Option<&InputClient>) -> Result<(), RuntimeError> {
    if let Some(client) = client {
        validate(client)?;
    }
    Ok(())
}
pub(super) fn admission_event(
    state: &super::SessionState,
    input: &super::events::Admitted,
    client: Option<&InputClient>,
) -> Result<Option<NewEvent>, RuntimeError> {
    let Some(client) = client else {
        return Ok(None);
    };
    if input.source != "user" || input.skill_command.is_some() || state.info.parent_id.is_some() {
        return Err(RuntimeError::Invalid(
            "Input-client presence requires an original root user input".into(),
        ));
    }
    Ok(Some(binding(
        &input.message_id,
        &state.info.directory,
        client,
    )))
}
pub(super) fn commit(
    inner: &super::Inner,
    state: &mut super::SessionState,
    events: Vec<NewEvent>,
    client: Option<&InputClient>,
) -> Result<Vec<cyber_store::StoredEvent>, RuntimeError> {
    let Some(client) = client else {
        return inner.commit_locked(state, events);
    };
    let (session, directory) = (state.info.id.clone(), canonical(&state.info.directory));
    inner
        .presence
        .with_input_admission(&session, &directory, client.client_id.as_deref(), |live| {
            if !live {
                return Err(RuntimeError::Invalid(
                    "Input client is not live in this Session and Location".into(),
                ));
            }
            inner.commit_locked(state, events)
        })
}
pub(super) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(BOUND, |data| {
            let binding: Binding =
                serde_json::from_value(data.clone()).map_err(|error| error.to_string())?;
            if !valid_id(&binding.message_id)
                || binding.directory.is_empty()
                || binding.directory.len() > 128 * 1024
            {
                return Err("Invalid input-client binding".into());
            }
            validate(&binding.client).map_err(|error| error.to_string())
        })
        .expect("valid input-client binding event");
}
pub(super) fn canonical(directory: &str) -> String {
    std::fs::canonicalize(directory)
        .map_or_else(|_| directory.to_owned(), |path| path.display().to_string())
}
pub(super) fn binding(message: &str, directory: &str, client: &InputClient) -> NewEvent {
    super::events::event(
        BOUND,
        &Binding {
            message_id: message.into(),
            directory: canonical(directory),
            client: client.clone(),
        },
    )
}
pub(super) fn attendance(
    store: &Store,
    session: &str,
    ids: &[String],
) -> Result<Attendance, StoreError> {
    let (session, ids) = (session.to_owned(), ids.to_vec());
    store.read(move |db| recorded(db, &session, &ids))
}
fn recorded(db: &Connection, session: &str, ids: &[String]) -> Result<Attendance, StoreError> {
    if ids.is_empty() {
        return Ok(Attendance::Legacy);
    }
    if ids.len() > 32
        || ids.iter().any(|id| !valid_id(id))
        || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len()
    {
        return Ok(Attendance::Unknown);
    }
    let mut authority = None;
    for id in ids {
        let current = bound_input(db, session, id)?;
        if authority
            .as_ref()
            .is_some_and(|previous| previous != &current)
        {
            return Ok(Attendance::Unknown);
        }
        authority = Some(current);
    }
    Ok(authority.unwrap_or(Attendance::Unknown))
}
fn bound_input(db: &Connection, session: &str, id: &str) -> Result<Attendance, StoreError> {
    let mut statement=db.prepare("SELECT CASE WHEN length(CAST(e.data AS BLOB))<=1048576 THEN e.data ELSE NULL END,
        json_extract(a.data,'$.digest') FROM event e LEFT JOIN event a ON a.aggregate_id=e.aggregate_id
        AND a.seq=e.seq-1 AND a.type='session.prompt.admitted.1'
        AND json_extract(a.data,'$.message_id')=?3 AND json_extract(a.data,'$.source')='user'
        WHERE e.aggregate_id=?1 AND e.type=?2 AND json_extract(e.data,'$.message_id')=?3 ORDER BY e.seq LIMIT 2")?;
    let records = statement
        .query_map(params![session, BOUND, id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if records.is_empty() {
        let scoped:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND type='session.prompt.admitted.1'
            AND json_extract(data,'$.message_id')=?2 AND instr(json_extract(data,'$.digest'),'+input-client:')>0)",params![session,id],|row|row.get(0))?;
        return Ok(if scoped {
            Attendance::Unknown
        } else {
            Attendance::Legacy
        });
    }
    if records.len() != 1 {
        return Ok(Attendance::Unknown);
    }
    let Some(binding) = records[0]
        .0
        .as_ref()
        .and_then(|raw| serde_json::from_str::<Binding>(raw).ok())
    else {
        return Ok(Attendance::Unknown);
    };
    let marker = format!(
        "+input-client:{}",
        serde_json::to_string(&binding.client).expect("client serializes")
    );
    if records[0]
        .1
        .as_ref()
        .is_none_or(|digest| !digest.contains(&marker))
        || validate(&binding.client).is_err()
        || binding.directory.is_empty()
        || binding.directory.len() > 128 * 1024
    {
        return Ok(Attendance::Unknown);
    }
    Ok(Attendance::Bound {
        session: session.into(),
        directory: binding.directory,
        client: binding.client.client_id,
    })
}
/// Preliminary tool-adapter check. Opening the request rechecks its actual Location.
pub(super) fn asker_attended(
    inner: &super::Inner,
    session: &str,
    message: &str,
    call: &str,
) -> bool {
    let (owner, step, tool) = (session.to_owned(), message.to_owned(), call.to_owned());
    let roots = inner
        .store
        .read(move |db| super::requests::input_owners::recorded(db, &owner, &step, &tool));
    let authority = roots.and_then(|ids| attendance(&inner.store, session, &ids));
    match authority {
        Ok(Attendance::Legacy) => inner
            .presence
            .session_attended(session, inner.options.interactive),
        Ok(authority @ Attendance::Bound { .. }) => {
            let Attendance::Bound {
                session, directory, ..
            } = &authority
            else {
                unreachable!()
            };
            inner.presence.with_request_attendance(
                &[InteractiveScope {
                    session_id: session.clone(),
                    directory: directory.clone(),
                }],
                inner.options.interactive,
                &authority,
                |live| live,
            )
        }
        _ => false,
    }
}

pub(super) fn last_seen(
    inner: &super::Inner,
    authority: &Attendance,
    legacy: &[super::presence::Watching],
    session: &str,
    directory: &str,
) -> Option<std::time::Instant> {
    if matches!(authority, Attendance::Legacy) {
        return inner.presence.last_seen(legacy);
    }
    inner.presence.with_request_attendance(
        &[InteractiveScope {
            session_id: session.into(),
            directory: canonical(directory),
        }],
        inner.options.interactive,
        authority,
        |live| live.then(std::time::Instant::now),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE event(aggregate_id TEXT,type TEXT,seq INTEGER,data TEXT)")
            .unwrap();
        db
    }
    fn insert(db: &Connection, session: &str, message: &str, client: Option<&str>) {
        let client = InputClient {
            client_id: client.map(str::to_owned),
        };
        let seq: i64 = db
            .query_row(
                "SELECT coalesce(max(seq),-1)+1 FROM event WHERE aggregate_id=?1",
                [session],
                |row| row.get(0),
            )
            .unwrap();
        let digest = format!(
            "base+input-client:{}",
            serde_json::to_string(&client).unwrap()
        );
        db.execute(
            "INSERT INTO event VALUES(?1,'session.prompt.admitted.1',?2,?3)",
            params![
                session,
                seq,
                serde_json::json!({"message_id":message,"source":"user","digest":digest})
                    .to_string()
            ],
        )
        .unwrap();
        let event = binding(message, "/repo", &client);
        db.execute(
            "INSERT INTO event VALUES(?1,?2,?3,?4)",
            params![session, BOUND, seq + 1, event.data.to_string()],
        )
        .unwrap();
    }
    #[test]
    fn complete_cohort_requires_one_immutable_client_and_unknown_never_becomes_legacy() {
        let db = db();
        insert(&db, "session", "a", Some("first"));
        insert(&db, "session", "b", Some("first"));
        assert!(matches!(
            recorded(&db, "session", &["a".into(), "b".into()]).unwrap(),
            Attendance::Bound {
                client: Some(_),
                ..
            }
        ));
        assert_eq!(
            recorded(&db, "session", &["a".into(), "legacy".into()]).unwrap(),
            Attendance::Unknown
        );
        insert(&db, "session", "c", Some("other"));
        assert_eq!(
            recorded(&db, "session", &["a".into(), "c".into()]).unwrap(),
            Attendance::Unknown
        );
        insert(&db, "session", "a", Some("first"));
        assert_eq!(
            recorded(&db, "session", &["a".into()]).unwrap(),
            Attendance::Unknown
        );
        assert_eq!(
            recorded(&db, "foreign", &["a".into()]).unwrap(),
            Attendance::Legacy
        );
        assert_eq!(
            recorded(&db, "session", &["b".into(), "b".into()]).unwrap(),
            Attendance::Unknown
        );
        db.execute("DELETE FROM event WHERE aggregate_id='session' AND type=?1 AND json_extract(data,'$.message_id')='b'",[BOUND]).unwrap();
        assert_eq!(
            recorded(&db, "session", &["b".into()]).unwrap(),
            Attendance::Unknown,
            "missing scoped evidence borrowed legacy attendance"
        );
    }
}
