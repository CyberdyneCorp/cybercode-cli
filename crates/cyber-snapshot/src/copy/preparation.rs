//! Metadata settlement of a positive runtime pre-effect cancellation.
use super::{Store, storage};
use cap_fs_ext::DirExt;
use cyber_server::runtime::{SnapshotOrigin, SnapshotPreparationReceipt};
pub(super) const FILE: &str = "preparation.json";
pub(super) const LIMIT: u64 = SnapshotPreparationReceipt::MAX_ENCODED_BYTES;

impl Store {
    pub(super) fn verify_preparation_origin(&self, origin: &SnapshotOrigin) -> Result<(), String> {
        self.verify_worker_namespace(origin)?;
        let directory = self
            .journals
            .open_dir_nofollow(&origin.binding.journal_id)
            .map_err(storage::fail)?;
        match directory.symlink_metadata("intent.json") {
            Ok(_) => {
                let (_, journal) = self.journal(&origin.binding.journal_id)?;
                if journal.origin.as_ref() != Some(origin)
                    || !journal.applied.is_empty()
                    || journal.acknowledged
                    || journal.settled_by.is_some()
                    || journal.completed && !journal.changes.is_empty()
                {
                    return Err("Intent contradicts positive pre-effect cancellation".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(storage::fail(error)),
        }
        Ok(())
    }
    pub(crate) fn acknowledge_preparation(
        &self,
        receipt: &SnapshotPreparationReceipt,
    ) -> Result<(), String> {
        self.verify_native_preparation(&receipt.lifetime)?;
        self.verify_consumed_resources(receipt)?;
        let origin = receipt
            .origin
            .as_ref()
            .ok_or("Cancellation has no journal origin")?;
        if receipt.source_admitted != Some(false)
            || origin.activity_id != receipt.activity_id
            || origin.session_id != receipt.lifetime.owner.session_id
            || origin.session_created_id != receipt.lifetime.owner.session_created_id
            || origin.binding.root_identity != receipt.lifetime.root_identity
        {
            return Err("Cancellation has no positive pre-effect proof".into());
        }
        self.verify_preparation_origin(origin)?;
        if let Some(existing) = self.preparation_cancellation(&origin.binding.journal_id)? {
            return if existing == *receipt {
                Ok(())
            } else {
                Err("Journal has another cancellation receipt".into())
            };
        }
        let bytes = serde_json::to_vec(receipt).map_err(storage::fail)?;
        if bytes.len() as u64 > LIMIT {
            return Err("Preparation cancellation record exceeds limit".into());
        }
        self.reserve(
            (bytes.len() as u64)
                .checked_mul(2)
                .ok_or("Snapshot size overflow")?,
        )?;
        let directory = self
            .journals
            .open_dir_nofollow(&origin.binding.journal_id)
            .map_err(storage::fail)?;
        self.verify_preparation_origin(origin)?;
        storage::atomic(&directory, FILE, &bytes)
    }
    pub(super) fn preparation_cancellation(
        &self,
        id: &str,
    ) -> Result<Option<SnapshotPreparationReceipt>, String> {
        let directory = self.journals.open_dir_nofollow(id).map_err(storage::fail)?;
        match directory.symlink_metadata(FILE) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage::fail(error)),
        }
        let receipt: SnapshotPreparationReceipt =
            serde_json::from_slice(&storage::read(&directory, FILE, LIMIT)?)
                .map_err(storage::fail)?;
        self.verify_native_preparation(&receipt.lifetime)?;
        self.verify_consumed_resources(&receipt)?;
        let origin = receipt
            .origin
            .as_ref()
            .ok_or("Cancellation lacks original journal binding")?;
        if receipt.source_admitted != Some(false)
            || origin.binding.journal_id != id
            || origin.activity_id != receipt.activity_id
            || receipt.lifetime.owner.activity_id != receipt.activity_id
            || origin.session_id != receipt.lifetime.owner.session_id
            || origin.session_created_id != receipt.lifetime.owner.session_created_id
        {
            return Err("Invalid preparation cancellation binding".into());
        }
        self.verify_preparation_origin(origin)?;
        Ok(Some(receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_server::runtime::{SnapshotBinding, SnapshotLifetime, SnapshotLifetimeRequest};
    fn setup() -> (
        tempfile::TempDir,
        Store,
        super::super::lifetime::Lifetime,
        SnapshotPreparationReceipt,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(
            &temp.path().join("data"),
            root.to_str().unwrap(),
            super::super::Options {
                max_file: 65536,
                max_total: 4 * 1024 * 1024,
                ignore: vec![],
                private_roots: vec![],
            },
        )
        .unwrap();
        let lifetime = store
            .reserve_lifetime(SnapshotLifetimeRequest {
                activity_id: "op_preparation".into(),
                session_id: "ses_fixture".into(),
                session_created_id: "evt_fixture".into(),
                directory: store.root.clone(),
                worktree_id: None,
            })
            .unwrap();
        let dir = storage::private_child(&store.journals, "restore_preparation").unwrap();
        let origin = SnapshotOrigin {
            version: 1,
            activity_id: "op_preparation".into(),
            session_id: "ses_fixture".into(),
            session_created_id: "evt_fixture".into(),
            worktree_id: None,
            binding: SnapshotBinding {
                directory: store.root.clone(),
                root_identity: store.root_id.clone(),
                namespace_identity: storage::dir_identity(&store.journals).unwrap(),
                journal_id: "restore_preparation".into(),
                journal_identity: storage::dir_identity(&dir).unwrap(),
                lock_identity: Some(store.lock_identity().unwrap()),
            },
        };
        let receipt = SnapshotPreparationReceipt {
            activity_id: origin.activity_id.clone(),
            request_id: "cancel-preparation".into(),
            confirmation: "a".repeat(64),
            lifetime: lifetime.binding().clone(),
            native_sequence: 1,
            session_sequence: 8,
            origin: Some(origin),
            source_admitted: Some(false),
            checkout_resources_digest: None,
        };
        (temp, store, lifetime, receipt)
    }
    #[test]
    fn origin_without_intent_is_acknowledged_without_source_scan_or_fabricated_completion() {
        let (temp, store, mut lifetime, receipt) = setup();
        let target = temp.path().join("owner-target");
        std::fs::write(&target, b"later owner bytes").unwrap();
        std::os::unix::fs::symlink(&target, store.root.join("source")).unwrap();
        assert!(store.ensure_recovered().is_err());
        drop(store);
        // A historical receipt still reconciles its original journal metadata;
        // publishing resources does not promote it to checkout acknowledgement.
        lifetime.bind_checkouts(&[]).unwrap();
        assert!(receipt.checkout_resources_digest.is_none());
        lifetime.acknowledge_preparation(&receipt).unwrap();
        lifetime.acknowledge_preparation(&receipt).unwrap();
        let store = Store::recover_existing(
            &temp.path().join("data"),
            temp.path().join("work").to_str().unwrap(),
            super::super::Options {
                max_file: 65536,
                max_total: 4 * 1024 * 1024,
                ignore: vec![],
                private_roots: vec![],
            },
        )
        .unwrap();
        store.ensure_recovered().unwrap();
        assert!(store.recovery().unwrap().is_empty());
        assert!(
            store.journal("restore_preparation").is_err(),
            "cancellation invented a restore intent"
        );
        assert!(
            std::fs::symlink_metadata(store.root.join("source"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(target).unwrap(), b"later owner bytes");
    }
    #[test]
    fn resource_digest_must_match_native_publication_before_metadata_acknowledgement() {
        let (temp, store, mut lifetime, mut receipt) = setup();
        let root = store.root.clone();
        let options = store.options.clone();
        let directory = store
            .journals
            .open_dir_nofollow("restore_preparation")
            .unwrap();
        drop(store);
        lifetime.bind_checkouts(&[]).unwrap();
        receipt.checkout_resources_digest = Some("0".repeat(64));
        assert!(lifetime.acknowledge_preparation(&receipt).is_err());
        assert!(directory.symlink_metadata(FILE).is_err());
        receipt.checkout_resources_digest =
            Some(receipt.lifetime.checkout_resources_digest(&[]).unwrap());
        lifetime.acknowledge_preparation(&receipt).unwrap();
        let recorded: SnapshotPreparationReceipt =
            serde_json::from_slice(&storage::read(&directory, FILE, LIMIT).unwrap()).unwrap();
        assert_eq!(recorded, receipt);
        let store =
            Store::recover_existing(&temp.path().join("data"), root.to_str().unwrap(), options)
                .unwrap();
        assert_eq!(
            store
                .preparation_cancellation("restore_preparation")
                .unwrap(),
            Some(receipt)
        );
        let mut corrupt = recorded;
        corrupt.checkout_resources_digest = Some("0".repeat(64));
        storage::atomic(&directory, FILE, &serde_json::to_vec(&corrupt).unwrap()).unwrap();
        assert!(
            store
                .preparation_cancellation("restore_preparation")
                .is_err()
        );
    }

    #[test]
    fn contradictory_intent_and_admitted_receipt_never_publish_cancellation_metadata() {
        let (_temp, store, _lifetime, mut receipt) = setup();
        receipt.source_admitted = Some(true);
        assert!(store.acknowledge_preparation(&receipt).is_err());
        receipt.source_admitted = Some(false);
        let dir = store
            .journals
            .open_dir_nofollow("restore_preparation")
            .unwrap();
        storage::create(&dir, "intent.json", b"corrupt intent evidence").unwrap();
        assert!(store.acknowledge_preparation(&receipt).is_err());
        assert!(dir.symlink_metadata(FILE).is_err());
        assert_eq!(
            storage::read(&dir, "intent.json", 100).unwrap(),
            b"corrupt intent evidence"
        );
    }
    #[test]
    fn cancellation_metadata_obeys_lowered_cap_and_retries_without_deleting_evidence() {
        let (_temp, mut store, _lifetime, receipt) = setup();
        store.options.max_total = 1;
        assert!(store.acknowledge_preparation(&receipt).is_err());
        assert!(store.ensure_recovered().is_err());
        store.options.max_total = 4 * 1024 * 1024;
        store.acknowledge_preparation(&receipt).unwrap();
        assert_eq!(
            store
                .preparation_cancellation("restore_preparation")
                .unwrap(),
            Some(receipt)
        );
    }
    #[test]
    fn native_admission_survives_an_older_database_phase_and_missing_phase_is_never_false_proof() {
        let (_temp, store, lifetime, receipt) = setup();
        let origin = receipt.origin.as_ref().unwrap();
        let dir = store
            .journals
            .open_dir_nofollow(&origin.binding.journal_id)
            .unwrap();
        let mut journal = store
            .prepare_journal(vec![], &origin.binding.journal_id, None, None, None)
            .unwrap();
        journal.origin = Some(origin.clone());
        journal.ownership =
            Some(super::super::ownership::Ownership::prepare(&store, &dir, &[]).unwrap());
        Store::record(&dir, &journal).unwrap();
        lifetime.admit_source(origin).unwrap();
        // A stale/rolled-back database receipt still says false. Native evidence
        // independently refuses cancellation and is never reset by another call.
        assert!(lifetime.verify_preparation(None).is_err());
        assert!(store.acknowledge_preparation(&receipt).is_err());
        assert!(dir.symlink_metadata(FILE).is_err());
        let workers = storage::existing_child(&store.namespace, "workers", true).unwrap();
        let phase = receipt.lifetime.source_phase.as_ref().unwrap();
        workers.remove_file(phase).unwrap();
        assert!(lifetime.verify_preparation(None).is_err());
        assert!(
            store
                .reserve_lifetime(receipt.lifetime.owner.clone())
                .is_err()
        );
        assert!(
            workers.symlink_metadata(phase).is_err(),
            "missing source phase was recreated"
        );
    }
}
