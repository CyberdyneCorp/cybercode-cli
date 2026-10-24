//! A transactional visibility projection makes a page proportional to its visible
//! messages, rather than to unrelated private project traffic.
use super::{Error, get};
use crate::model::{Agent, Message, MessageTarget, Participant};
use rusqlite::{Connection, params};
use std::collections::BTreeSet;

pub(super) fn create(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS ch_visibility(project TEXT NOT NULL,participant TEXT NOT NULL,message TEXT NOT NULL,PRIMARY KEY(project,participant,message));
         CREATE INDEX IF NOT EXISTS ch_visibility_message ON ch_visibility(message);",
    )
}

/// Schema 1 contains authoritative messages and immutable agent owners. Rebuild
/// visibility in the same transaction that advances the schema version.
pub(super) fn backfill(db: &Connection) -> rusqlite::Result<()> {
    create(db)?;
    db.execute_batch(
        "INSERT OR IGNORE INTO ch_visibility
         SELECT project,'project',id FROM ch_objects WHERE kind='message' AND json_extract(body,'$.target.kind')='project';
         INSERT OR IGNORE INTO ch_visibility
         SELECT project,json_extract(body,'$.author.kind')||':'||json_extract(body,'$.author.id'),id FROM ch_objects WHERE kind='message' AND json_extract(body,'$.target.kind')='direct';
         INSERT OR IGNORE INTO ch_visibility
         SELECT project,json_extract(body,'$.target.participant.kind')||':'||json_extract(body,'$.target.participant.id'),id FROM ch_objects WHERE kind='message' AND json_extract(body,'$.target.kind')='direct';
         INSERT OR IGNORE INTO ch_visibility
         SELECT m.project,'human:'||json_extract(a.body,'$.owner'),m.id FROM ch_objects m JOIN ch_objects a ON a.kind='agent' AND a.id=json_extract(m.body,'$.author.id') AND a.project=m.project WHERE m.kind='message' AND json_extract(m.body,'$.target.kind')='direct' AND json_extract(m.body,'$.author.kind')='agent';
         INSERT OR IGNORE INTO ch_visibility
         SELECT m.project,'human:'||json_extract(a.body,'$.owner'),m.id FROM ch_objects m JOIN ch_objects a ON a.kind='agent' AND a.id=json_extract(m.body,'$.target.participant.id') AND a.project=m.project WHERE m.kind='message' AND json_extract(m.body,'$.target.kind')='direct' AND json_extract(m.body,'$.target.participant.kind')='agent';",
    )
}

pub(super) fn index(db: &Connection, message: &Message) -> Result<(), Error> {
    let mut keys = BTreeSet::new();
    match &message.target {
        MessageTarget::Project => {
            keys.insert("project".to_owned());
        }
        MessageTarget::Direct(target) => {
            for participant in [&message.author, target] {
                keys.insert(participant_key(participant));
                if let Participant::Agent(id) = participant {
                    let agent: Agent = get(db, "agent", id.as_str())?;
                    if agent.project != message.project {
                        return Err(Error::forbidden());
                    }
                    keys.insert(format!("human:{}", agent.owner));
                }
            }
        }
    }
    // A projection has at most four private keys. Its message index keeps updates
    // (including expiry edits) independent of the rest of project history.
    db.execute(
        "DELETE FROM ch_visibility WHERE message=?1",
        [message.id.as_str()],
    )?;
    for key in keys {
        db.execute(
            "INSERT INTO ch_visibility(project,participant,message) VALUES(?1,?2,?3)",
            params![message.project.as_str(), key, message.id.as_str()],
        )?;
    }
    Ok(())
}

pub(crate) fn participant_key(participant: &Participant) -> String {
    match participant {
        Participant::Human(id) => format!("human:{id}"),
        Participant::Agent(id) => format!("agent:{id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ids::*,
        model::*,
        storage::{self, Actor, Database},
    };
    use std::sync::Arc;
    fn message(project: &ProjectId, author: Participant, target: MessageTarget) -> Message {
        Message {
            id: MessageId::new(),
            project: project.clone(),
            author,
            owner: MemberId::new(),
            membership_kind: MembershipKind::Internal,
            member_generation: 1,
            depth: 0,
            hops: 1,
            target,
            reply_to: None,
            text: "history".into(),
            created_at: storage::now(),
            expires_at: storage::now() + 604800,
        }
    }
    fn rows(db: &Connection) -> Result<Vec<(String, String, String)>, Error> {
        let mut stmt=db.prepare("SELECT project,participant,message FROM ch_visibility ORDER BY project,participant,message")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Error::from)
    }
    #[test]
    fn schema_one_backfills_all_visibility_kinds_and_preserves_private_agent_owners() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hub.sqlite");
        let db = Database::open(&path).unwrap();
        let project = ProjectId::new();
        let alice = MemberId::new();
        let bob = MemberId::new();
        let source = Agent {
            id: AgentId::new(),
            project: project.clone(),
            owner: alice.clone(),
            runtime: RuntimeId::new(),
            name: "retired source".into(),
            kind: MembershipKind::Internal,
            active: false,
            online: false,
        };
        let target = Agent {
            id: AgentId::new(),
            project: project.clone(),
            owner: bob.clone(),
            runtime: RuntimeId::new(),
            name: "target".into(),
            kind: MembershipKind::Internal,
            active: true,
            online: true,
        };
        let private = message(
            &project,
            Participant::Agent(source.id.clone()),
            MessageTarget::Direct(Participant::Agent(target.id.clone())),
        );
        let public = message(
            &project,
            Participant::Human(alice.clone()),
            MessageTarget::Project,
        );
        let direct = message(
            &project,
            Participant::Human(alice.clone()),
            MessageTarget::Direct(Participant::Human(bob.clone())),
        );
        let p = project.clone();
        let before=db.write(move|db|{
            storage::put(db,"agent",source.id.as_str(),p.as_str(),&source)?;storage::put(db,"agent",target.id.as_str(),p.as_str(),&target)?;
            for m in [private,public,direct] { storage::put(db,"message",m.id.as_str(),p.as_str(),&m)?; }
            let indexed=rows(db)?;
            assert_eq!(indexed.len(),7);
            assert!(indexed.iter().any(|(_,key,_)|key==&format!("human:{alice}")));
            db.execute_batch("DROP TABLE ch_visibility;UPDATE ch_meta SET value='1' WHERE key='schema_version';")?;
            Ok(indexed)
        }).unwrap();
        drop(db);
        let reopened = Database::open(&path).unwrap();
        assert_eq!(reopened.read(rows).unwrap(), before);
        let version: String = reopened
            .read(|db| {
                Ok(db.query_row(
                    "SELECT value FROM ch_meta WHERE key='schema_version'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(version, "2");
    }
    #[test]
    fn message_updates_replace_visibility_and_failed_indexing_rolls_back_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("hub.sqlite")).unwrap();
        let p = ProjectId::new();
        let alice = MemberId::new();
        let bob = MemberId::new();
        let mallory = MemberId::new();
        let mut m = message(
            &p,
            Participant::Human(alice),
            MessageTarget::Direct(Participant::Human(bob.clone())),
        );
        let project = p.clone();
        let copy = m.clone();
        db.write(move |db| storage::put(db, "message", copy.id.as_str(), project.as_str(), &copy))
            .unwrap();
        m.target = MessageTarget::Direct(Participant::Human(mallory.clone()));
        let project = p.clone();
        let copy = m.clone();
        db.write(move |db| storage::put(db, "message", copy.id.as_str(), project.as_str(), &copy))
            .unwrap();
        let indexed = db.read(rows).unwrap();
        assert!(
            indexed
                .iter()
                .all(|(_, key, _)| key != &format!("human:{bob}"))
        );
        assert!(
            indexed
                .iter()
                .any(|(_, key, _)| key == &format!("human:{mallory}"))
        );
        let bad = message(
            &p,
            Participant::Human(MemberId::new()),
            MessageTarget::Direct(Participant::Agent(AgentId::new())),
        );
        let id = bad.id.clone();
        assert!(
            db.write(move |db| storage::put(db, "message", bad.id.as_str(), p.as_str(), &bad))
                .is_err()
        );
        assert!(
            db.read(move |db| storage::get::<Message>(db, "message", id.as_str()))
                .is_err()
        );
    }
    #[tokio::test]
    async fn visible_page_skips_ten_thousand_newer_private_messages_using_scoped_indexes() {
        let fixture = crate::fixture::Fixture::start().await;
        let mut verifier = fixture.client().await;
        let proof = Arc::new(
            verifier
                .authorize_human(&fixture.access(), &fixture.service(), "hub")
                .await
                .unwrap(),
        );
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("hub.sqlite")).unwrap();
        let project = ProjectId::new();
        let alice = Member {
            id: MemberId::new(),
            project: project.clone(),
            issuer: fixture.state.issuer.clone(),
            subject: "alice".into(),
            name: "Alice".into(),
            role: Role::Member,
            kind: MembershipKind::Internal,
            generation: 1,
            active: true,
        };
        let bob = MemberId::new();
        let actor = Actor {
            proof,
            member: Some(alice.clone()),
            agent: None,
        };
        let p = project.clone();
        db.write(move |db| {
            for number in 1u128..=10002 {
                let mut m = message(
                    &p,
                    Participant::Human(if number == 1 {
                        alice.id.clone()
                    } else {
                        bob.clone()
                    }),
                    if number == 1 {
                        MessageTarget::Project
                    } else {
                        MessageTarget::Direct(Participant::Human(bob.clone()))
                    },
                );
                m.id = format!("cmsg_{}", ulid::Ulid::from(number))
                    .parse()
                    .unwrap();
                m.text = number.to_string();
                storage::put(db, "message", m.id.as_str(), p.as_str(), &m)?;
            }
            Ok(())
        })
        .unwrap();
        let p = project.clone();
        let a = actor.clone();
        let page = db
            .read(move |db| crate::server::message_page(db, &a, p.as_str(), None))
            .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].text, "1");
        let p = project;
        let page = db
            .read(move |db| {
                crate::server::message_page(
                    db,
                    &actor,
                    p.as_str(),
                    Some("cmsg_00000000000000000000000000"),
                )
            })
            .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].text, "1");
    }
}
