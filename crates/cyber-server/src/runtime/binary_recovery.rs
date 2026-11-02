//! A document's actual exclusive lock proves that a prior filesystem-only worker
//! cannot still install bytes through that document. Generic native processes need
//! their own acknowledgement and are never settled by this proof.
use super::{Runtime, RuntimeError, activity};
use cyber_core::binary::editor::RecoveryProof;
use cyber_store::{Expected, NewEvent};

impl Runtime {
    pub(crate) fn recover_binary_activity(
        &self,
        proof: RecoveryProof<'_>,
    ) -> Result<usize, RuntimeError> {
        let storage = self
            .binary_documents()
            .storage()
            .map_err(|_| RuntimeError::Conflict("Binary recovery storage unavailable".into()))?;
        proof
            .validate(&storage)
            .map_err(|_| RuntimeError::Conflict("Binary recovery namespace changed".into()))?;
        let session = proof.owner().session_id.clone();
        let document = proof.document_id().to_owned();
        if self
            .inner
            .activities
            .lock()
            .map_err(|_| RuntimeError::Conflict("Binary activity registry unavailable".into()))?
            .values()
            .any(|control| {
                control.source == session && control.binary_document.as_deref() == Some(&document)
            })
        {
            return Err(RuntimeError::Conflict(
                "The binary filesystem worker is still active".into(),
            ));
        }
        let rows = self.inner.store.read(move |db| {
            let mut query = db.prepare("SELECT aggregate_id,seq,data,(SELECT data FROM event origin WHERE origin.aggregate_id=e.aggregate_id ORDER BY seq LIMIT 1) FROM event e WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.binary_document')=?2 AND json_extract(data,'$.status')<>'settled' AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq) LIMIT 4097")?;
            Ok(query.query_map([session, document], |row| Ok((row.get::<_,String>(0)?, row.get::<_,i64>(1)?, row.get::<_,String>(2)?, row.get::<_,String>(3)?)))?.collect::<Result<Vec<_>,_>>()?)
        })?;
        if rows.len() > 4096 {
            return Err(RuntimeError::Conflict(
                "Binary recovery activity limit exceeded".into(),
            ));
        }
        let count = rows.len();
        for (id, sequence, data, origin) in rows {
            let mut record: activity::Record = serde_json::from_str(&data)
                .map_err(|error| RuntimeError::Corrupt(error.to_string()))?;
            let origin: activity::Record = serde_json::from_str(&origin)
                .map_err(|error| RuntimeError::Corrupt(error.to_string()))?;
            if origin.session_id != proof.owner().session_id
                || origin.binary_document.as_deref() != Some(proof.document_id())
            {
                return Err(RuntimeError::Corrupt(
                    "Binary activity origin differs from its document".into(),
                ));
            }
            proof
                .validate(&storage)
                .map_err(|_| RuntimeError::Conflict("Binary recovery namespace changed".into()))?;
            record.status = "settled".into();
            record.phase = "filesystem_recovered_document_locked".into();
            record.admission_bindings = None;
            self.inner.store.append(
                &id,
                Expected::Seq(sequence),
                vec![NewEvent::new(
                    activity::CHANGED,
                    serde_json::to_value(record)
                        .map_err(|error| RuntimeError::Corrupt(error.to_string()))?,
                )],
            )?;
        }
        Ok(count)
    }
}
