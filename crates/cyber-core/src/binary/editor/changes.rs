//! Native acknowledgement evidence for the shared observation producer. This
//! type cannot be deserialized or manufactured from an HTTP/plugin payload.
use super::*;
use crate::hooks::file_observations::Stamp;

pub struct AcknowledgedChange {
    key: String,
    owner: Owner,
    resource: super::super::Resource,
    version: crate::binary::receipts::Version,
    target: Target,
    storage: crate::binary::storage::StorageRoot,
    _pins: native::Pins,
}
impl AcknowledgedChange {
    pub fn belongs_to(&self, storage: &crate::binary::storage::StorageRoot) -> Result<(), Error> {
        self.storage.same_namespace(storage)
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn owner(&self) -> &Owner {
        &self.owner
    }
    pub fn resource(&self) -> &super::super::Resource {
        &self.resource
    }
    pub fn version(&self) -> &crate::binary::receipts::Version {
        &self.version
    }
    /// Metadata-only confirmation for the existing native scanner. This is not
    /// writer attribution or permission to execute a hook. A superseded source
    /// refuses confirmation rather than fabricating an observation of old bytes.
    pub fn observed_stamp(&self) -> Result<Stamp, Error> {
        use std::os::unix::fs::MetadataExt;
        let (_, source) = self.target.validate()?;
        let metadata = source.metadata().map_err(|_| Error::Io)?;
        Ok(Stamp {
            length: metadata.len(),
            modified_ns: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|time| time.as_nanos().try_into().ok()),
            identity: Some((metadata.dev(), metadata.ino())),
        })
    }
}
impl Editor {
    /// A known earlier save can finish metadata publication after explicit
    /// recovery. Unsaved edits and unknown native intents are never candidates.
    pub fn pending_notification_change(&mut self) -> Result<Option<AcknowledgedChange>, Error> {
        let pending = self
            .record
            .operations
            .iter()
            .rev()
            .find(|operation| {
                operation.notification_pending
                    && operation.outcome == Outcome::Acknowledged
                    && operation.file_effect == FileEffect::Changed
            })
            .map(|operation| operation.id.clone());
        match pending {
            Some(id) => self.acknowledged_change(&id),
            None => Ok(None),
        }
    }
    /// Retain durable source-version evidence before handing acknowledgement to
    /// another producer. Unknown/conflict/no-effect operations never mint proof.
    pub fn acknowledged_change(&mut self, id: &str) -> Result<Option<AcknowledgedChange>, Error> {
        self.publish_last_version()?;
        let Some(operation) = self.lookup_operation(id)? else {
            return Ok(None);
        };
        if operation.outcome != Outcome::Acknowledged
            || operation.file_effect != FileEffect::Changed
        {
            return Ok(None);
        }
        let version = format!(
            "ver_{}",
            signature(&(
                "binary-version-v1",
                self.store.document_id(),
                &operation.id,
                &operation.payload_sha256
            ))?
        );
        let evidence =
            versions::read_evidence(self.store.directory(), self.store.document_id(), &version)?;
        if evidence.owner != self.record.owner
            || evidence.operation.payload_sha256 != operation.payload_sha256
        {
            return Err(Error::Changed);
        }
        let pins = evidence.target.pins()?;
        Ok(Some(AcknowledgedChange {
            key: version,
            owner: self.record.owner.clone(),
            resource: self.resource(),
            version: evidence.info,
            target: evidence.target,
            storage: self.store.storage_root()?,
            _pins: pins,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_durable_changed_acknowledgements_mint_source_change_evidence() {
        let (_temp, mut editor, path, owner) = super::super::tests::setup();
        assert!(editor.pending_notification_change().unwrap().is_none());
        assert!(editor.acknowledged_change("absent").unwrap().is_none());
        super::super::tests::patch(&mut editor, "patch_one");
        assert!(editor.pending_notification_change().unwrap().is_none());
        assert!(editor.acknowledged_change("patch_one").unwrap().is_none());
        let prepared = super::super::tests::ready(&mut editor, "saved");
        assert!(editor.acknowledged_change("saved").unwrap().is_none());
        editor.commit_save(*prepared).unwrap();
        let change = editor.acknowledged_change("saved").unwrap().unwrap();
        assert_eq!(
            editor.pending_notification_change().unwrap().unwrap().key(),
            change.key()
        );
        assert_eq!(change.owner(), &owner);
        assert_eq!(change.resource().path(), path.canonicalize().unwrap());
        assert_eq!(change.version().after_length, Offset(4));
        assert_eq!(change.key(), change.version().version_id);
        assert_eq!(change.observed_stamp().unwrap().length, 4);
        assert_eq!(
            change.key(),
            editor.acknowledged_change("saved").unwrap().unwrap().key()
        );
        std::fs::write(&path, [9, 8, 7, 6]).unwrap();
        assert!(change.observed_stamp().is_err());
        assert_eq!(std::fs::read(path).unwrap(), [9, 8, 7, 6]);
    }
    #[test]
    fn acknowledgement_proof_belongs_only_to_its_configured_private_namespace() {
        let (temp, mut editor, _, _) = super::super::tests::setup();
        super::super::tests::patch(&mut editor, "patch_one");
        let prepared = super::super::tests::ready(&mut editor, "saved");
        editor.commit_save(*prepared).unwrap();
        let change = editor.acknowledged_change("saved").unwrap().unwrap();
        let storage =
            crate::binary::storage::StorageRoot::prepare(&temp.path().join("data")).unwrap();
        change.belongs_to(&storage).unwrap();
        let other = tempfile::tempdir().unwrap();
        let other = crate::binary::storage::StorageRoot::prepare(other.path()).unwrap();
        assert!(change.belongs_to(&other).is_err());
    }
}
