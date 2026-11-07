//! A worker-specific native lock precedes every durable runtime reservation.
use super::{Options, Store, storage};
use cyber_server::runtime::{SnapshotLifetime, SnapshotLifetimeBinding, SnapshotLifetimeRequest};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const RECORD_LIMIT: u64 = 16 * 1024;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcePhase {
    lifetime: SnapshotLifetimeBinding,
    admitted: bool,
    origin: Option<cyber_server::runtime::SnapshotOrigin>,
}

pub(crate) struct Lifetime {
    binding: SnapshotLifetimeBinding,
    file: File,
    data: PathBuf,
    options: Options,
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl SnapshotLifetime for Lifetime {
    fn binding(&self) -> &SnapshotLifetimeBinding {
        &self.binding
    }
    fn verify(&self) -> Result<(), String> {
        if storage::identity(&self.file)? != self.binding.file_identity {
            return Err("Snapshot lifetime descriptor changed".into());
        }
        storage::validate_lock(&self.file)?;
        let store = Store::existing(
            &self.data,
            &self.binding.owner.directory.to_string_lossy(),
            self.options.clone(),
        )?
        .ok_or("Snapshot lifetime namespace is missing")?;
        store.verify_lifetime(&self.binding)
    }
    fn verify_preparation(
        &self,
        origin: Option<&cyber_server::runtime::SnapshotOrigin>,
    ) -> Result<(), String> {
        self.verify()?;
        let store = Store::existing(
            &self.data,
            &self.binding.owner.directory.to_string_lossy(),
            self.options.clone(),
        )?
        .ok_or("Missing preparation namespace")?;
        store.verify_native_preparation(&self.binding)?;
        if let Some(origin) = origin {
            if origin.activity_id != self.binding.owner.activity_id
                || origin.session_id != self.binding.owner.session_id
                || origin.session_created_id != self.binding.owner.session_created_id
                || origin.worktree_id != self.binding.owner.worktree_id
            {
                return Err("Foreign preparation origin".into());
            }
            let store = Store::existing(
                &self.data,
                &self.binding.owner.directory.to_string_lossy(),
                self.options.clone(),
            )?
            .ok_or("Missing preparation namespace")?;
            store.verify_preparation_origin(origin)?;
        }
        Ok(())
    }
    fn admit_source(&self, origin: &cyber_server::runtime::SnapshotOrigin) -> Result<(), String> {
        self.verify()?;
        let store = Store::existing(
            &self.data,
            &self.binding.owner.directory.to_string_lossy(),
            self.options.clone(),
        )?
        .ok_or("Missing source-admission namespace")?;
        store.record_source_admission(&self.binding, origin)
    }
    fn acknowledge_preparation(
        &mut self,
        receipt: &cyber_server::runtime::SnapshotPreparationReceipt,
    ) -> Result<(), String> {
        if receipt.lifetime != self.binding {
            return Err("Foreign preparation receipt".into());
        }
        self.verify_preparation(receipt.origin.as_ref())?;
        if receipt.origin.is_some() {
            let store = Store::recover_existing(
                &self.data,
                &self.binding.owner.directory.to_string_lossy(),
                self.options.clone(),
            )?;
            store.acknowledge_preparation(receipt)?;
        }
        Ok(())
    }
}

fn lock(file: &File) -> Result<(), String> {
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => "Snapshot worker is still active".into(),
        error => error.to_string(),
    })
}

fn valid_owner(owner: &SnapshotLifetimeRequest) -> Result<(), String> {
    if owner.directory.to_str().is_none()
        || !owner.activity_id.starts_with("op_")
        || owner.activity_id.len() > 256
        || !owner
            .activity_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || [&owner.session_id, &owner.session_created_id]
            .iter()
            .any(|id| id.is_empty() || id.len() > 256)
    {
        return Err("Invalid snapshot lifetime owner".into());
    }
    Ok(())
}

impl Store {
    pub(crate) fn reserve_lifetime(
        &self,
        owner: SnapshotLifetimeRequest,
    ) -> Result<Lifetime, String> {
        self.reserve_lifetime_before_publish(owner, || Ok(()))
    }
    fn reserve_lifetime_before_publish(
        &self,
        owner: SnapshotLifetimeRequest,
        before_publish: impl FnOnce() -> Result<(), String>,
    ) -> Result<Lifetime, String> {
        valid_owner(&owner)?;
        if owner.directory != self.root {
            return Err("Snapshot lifetime Location changed".into());
        }
        let directory = storage::private_child(&self.namespace, "workers")?;
        if directory.symlink_metadata(&owner.activity_id).is_ok() {
            let bytes = storage::read(&directory, &owner.activity_id, RECORD_LIMIT)?;
            let binding: SnapshotLifetimeBinding =
                serde_json::from_slice(&bytes).map_err(storage::fail)?;
            if binding.owner != owner {
                return Err("Snapshot lifetime owner changed".into());
            }
            self.verify_native_preparation(&binding)?;
            return self.lock_lifetime(&binding);
        }
        let mut binding = SnapshotLifetimeBinding {
            version: 2,
            owner,
            root_identity: self.root_id.clone(),
            namespace_identity: storage::dir_identity(&self.namespace)?,
            directory_identity: storage::dir_identity(&directory)?,
            file_identity: "0".repeat(storage::IDENTITY_MAX_BYTES),
            source_phase: Some(format!("phase_{}", ulid::Ulid::new())),
        };
        let phase_budget = serde_json::to_vec(&SourcePhase {
            lifetime: binding.clone(),
            admitted: false,
            origin: None,
        })
        .map_err(storage::fail)?
        .len() as u64;
        let binding_budget = serde_json::to_vec(&binding).map_err(storage::fail)?.len() as u64;
        let budget = binding_budget
            .checked_add(phase_budget)
            .ok_or("Snapshot lifetime size overflow")?;
        if binding_budget > RECORD_LIMIT || phase_budget > RECORD_LIMIT {
            return Err("Snapshot lifetime record exceeds limit".into());
        }
        self.reserve(budget)?;
        // A crash during construction retains an unreferenced private stage;
        // the stable owner name is published only with a complete durable record.
        let stage = format!("preparing_{}", ulid::Ulid::new());
        let mut file = storage::create(&directory, &stage, b"")?;
        lock(&file)?;
        binding.file_identity = storage::identity(&file)?;
        let bytes = serde_json::to_vec(&binding).map_err(storage::fail)?;
        file.write_all(&bytes).map_err(storage::fail)?;
        storage::sync_file(&file)?;
        let phase = SourcePhase {
            lifetime: binding.clone(),
            admitted: false,
            origin: None,
        };
        storage::create(
            &directory,
            binding
                .source_phase
                .as_deref()
                .ok_or("Missing source phase")?,
            &serde_json::to_vec(&phase).map_err(storage::fail)?,
        )?;
        storage::sync(&directory)?;
        before_publish()?;
        rustix::fs::renameat_with(
            &directory,
            &stage,
            &directory,
            &binding.owner.activity_id,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(storage::fail)?;
        storage::sync(&directory)?;
        Ok(Lifetime {
            binding,
            file,
            data: self.data_path.clone(),
            options: self.options.clone(),
        })
    }

    /// Reacquire only a persisted exact lock. Never create/replace missing proof.
    pub(crate) fn lock_lifetime(
        &self,
        binding: &SnapshotLifetimeBinding,
    ) -> Result<Lifetime, String> {
        self.verify_lifetime(binding)?;
        let directory = storage::existing_child(&self.namespace, "workers", true)?;
        let file = storage::open(&directory, Path::new(&binding.owner.activity_id))?;
        storage::validate_lock(&file)?;
        lock(&file)?;
        self.verify_lifetime(binding)?;
        if storage::identity(&file)? != binding.file_identity {
            return Err("Snapshot lifetime lock replaced".into());
        }
        Ok(Lifetime {
            binding: binding.clone(),
            file,
            data: self.data_path.clone(),
            options: self.options.clone(),
        })
    }

    fn verify_lifetime(&self, binding: &SnapshotLifetimeBinding) -> Result<(), String> {
        valid_owner(&binding.owner)?;
        self.verify_root()?;
        if !matches!(binding.version, 1 | 2)
            || binding.owner.directory != self.root
            || binding.root_identity != self.root_id
            || binding.namespace_identity != storage::dir_identity(&self.namespace)?
        {
            return Err("Snapshot lifetime namespace changed".into());
        }
        let directory = storage::existing_child(&self.namespace, "workers", true)?;
        let file = storage::open(&directory, Path::new(&binding.owner.activity_id))?;
        storage::validate_lock(&file)?;
        let mut bytes = Vec::new();
        file.try_clone()
            .map_err(storage::fail)?
            .take(RECORD_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(storage::fail)?;
        if bytes.len() as u64 > RECORD_LIMIT {
            return Err("Snapshot lifetime record exceeds limit".into());
        }
        let observed: SnapshotLifetimeBinding =
            serde_json::from_slice(&bytes).map_err(storage::fail)?;
        if binding.directory_identity != storage::dir_identity(&directory)?
            || binding.file_identity != storage::identity(&file)?
            || observed != *binding
        {
            return Err("Snapshot lifetime binding changed".into());
        }
        Ok(())
    }

    fn read_source_phase(&self, binding: &SnapshotLifetimeBinding) -> Result<SourcePhase, String> {
        self.verify_lifetime(binding)?;
        let name = binding
            .source_phase
            .as_deref()
            .ok_or("Legacy worker has no native source phase")?;
        if binding.version != 2
            || !name.starts_with("phase_")
            || name.len() > 128
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err("Unsupported native source phase".into());
        }
        let directory = storage::existing_child(&self.namespace, "workers", true)?;
        let file = storage::open(&directory, Path::new(name))?;
        storage::validate_lock(&file)?;
        let mut bytes = Vec::new();
        file.take(cyber_server::runtime::SnapshotPreparationReceipt::MAX_ENCODED_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(storage::fail)?;
        if bytes.len() as u64 > cyber_server::runtime::SnapshotPreparationReceipt::MAX_ENCODED_BYTES
        {
            return Err("Native source phase exceeds limit".into());
        }
        let phase: SourcePhase = serde_json::from_slice(&bytes).map_err(storage::fail)?;
        if phase.lifetime != *binding || phase.admitted != phase.origin.is_some() {
            return Err("Native source phase changed owner".into());
        }
        Ok(phase)
    }
    pub(super) fn verify_native_preparation(
        &self,
        binding: &SnapshotLifetimeBinding,
    ) -> Result<(), String> {
        if self.read_source_phase(binding)?.admitted {
            return Err("Native source effects may already have been admitted".into());
        }
        Ok(())
    }
    fn record_source_admission(
        &self,
        binding: &SnapshotLifetimeBinding,
        origin: &cyber_server::runtime::SnapshotOrigin,
    ) -> Result<(), String> {
        let phase = self.read_source_phase(binding)?;
        if origin.activity_id != binding.owner.activity_id
            || origin.session_id != binding.owner.session_id
            || origin.session_created_id != binding.owner.session_created_id
            || origin.worktree_id != binding.owner.worktree_id
        {
            return Err("Foreign native source admission".into());
        }
        self.verify_worker_namespace(origin)?;
        let (_, journal) = self.journal(&origin.binding.journal_id)?;
        if journal.origin.as_ref() != Some(origin) || journal.ownership.is_none() {
            return Err("Source admission lacks prepared native intent".into());
        }
        if phase.admitted {
            return if phase.origin.as_ref() == Some(origin) {
                Ok(())
            } else {
                Err("Native source origin changed".into())
            };
        }
        let phase = SourcePhase {
            lifetime: binding.clone(),
            admitted: true,
            origin: Some(origin.clone()),
        };
        let bytes = serde_json::to_vec(&phase).map_err(storage::fail)?;
        if bytes.len() as u64 > cyber_server::runtime::SnapshotPreparationReceipt::MAX_ENCODED_BYTES
        {
            return Err("Native source phase exceeds limit".into());
        }
        self.reserve(
            (bytes.len() as u64)
                .checked_mul(2)
                .ok_or("Snapshot source size overflow")?,
        )?;
        let directory = storage::existing_child(&self.namespace, "workers", true)?;
        storage::atomic(
            &directory,
            binding
                .source_phase
                .as_deref()
                .ok_or("Missing source phase")?,
            &bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options() -> Options {
        Options {
            max_file: 65536,
            max_total: 4 * 1024 * 1024,
            ignore: vec![],
            private_roots: vec![],
        }
    }
    fn request(root: &Path) -> SnapshotLifetimeRequest {
        SnapshotLifetimeRequest {
            activity_id: "op_lifetime_fixture".into(),
            session_id: "ses_fixture".into(),
            session_created_id: "evt_fixture".into(),
            directory: root.canonicalize().unwrap(),
            worktree_id: None,
        }
    }
    fn store(root: &Path) -> Store {
        std::fs::create_dir_all(root.join("work")).unwrap();
        Store::open(
            &root.join("data"),
            root.join("work").to_str().unwrap(),
            options(),
        )
        .unwrap()
    }

    #[test]
    fn exact_lifetime_lock_retains_privacy_quota_and_rejects_replacement_or_missing_proof() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let store = store(temp.path());
        let lifetime = store.reserve_lifetime(request(&store.root)).unwrap();
        let binding = lifetime.binding().clone();
        let path = temp.path().join("data/snapshot/global");
        let namespace = std::fs::read_dir(path)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let file = namespace.join("workers").join(&binding.owner.activity_id);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(store.usage().unwrap() >= std::fs::metadata(&file).unwrap().len());
        assert!(store.lock_lifetime(&binding).is_err());
        lifetime.verify().unwrap();
        let alias = namespace.join("lifetime_alias");
        std::fs::hard_link(&file, &alias).unwrap();
        assert!(lifetime.verify().is_err());
        assert!(store.lock_lifetime(&binding).is_err());
        std::fs::remove_file(alias).unwrap();
        lifetime.verify().unwrap();
        drop(lifetime);
        let recovered = store.lock_lifetime(&binding).unwrap();
        let bytes = std::fs::read(&file).unwrap();
        let replacement = namespace.join("replacement");
        std::fs::write(&replacement, bytes).unwrap();
        std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::rename(replacement, &file).unwrap();
        assert!(recovered.verify().is_err());
        assert!(store.lock_lifetime(&binding).is_err());
        std::fs::remove_file(&file).unwrap();
        assert!(store.lock_lifetime(&binding).is_err());
        assert!(!file.exists());
    }

    #[test]
    fn legacy_lifetime_stays_inspectable_but_never_grants_preparation_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(temp.path());
        let lifetime = store.reserve_lifetime(request(&store.root)).unwrap();
        let mut binding = lifetime.binding().clone();
        drop(lifetime);
        // Reproduce a genuine version-1 record in the same private inode. It
        // has valid lifetime ownership, but predates native source-phase proof.
        binding.version = 1;
        binding.source_phase = None;
        let workers = storage::existing_child(&store.namespace, "workers", true).unwrap();
        let mut file = workers
            .open_with(
                &binding.owner.activity_id,
                cap_std::fs::OpenOptions::new().write(true).truncate(true),
            )
            .unwrap();
        file.write_all(&serde_json::to_vec(&binding).unwrap())
            .unwrap();
        file.sync_all().unwrap();
        let legacy = store.lock_lifetime(&binding).unwrap();
        legacy.verify().unwrap();
        assert!(legacy.verify_preparation(None).is_err());
        drop(legacy);
        assert!(store.reserve_lifetime(binding.owner.clone()).is_err());
        let retained: SnapshotLifetimeBinding = serde_json::from_slice(
            &storage::read(&workers, &binding.owner.activity_id, RECORD_LIMIT).unwrap(),
        )
        .unwrap();
        assert_eq!(retained, binding);
    }

    #[test]
    fn lifetime_cap_refuses_before_creating_an_owner_file_and_gc_retains_preparation() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = store(temp.path());
        store.options.max_total = 1;
        assert!(store.reserve_lifetime(request(&store.root)).is_err());
        let workers = storage::existing_child(&store.namespace, "workers", true).unwrap();
        assert!(storage::names(&workers, 10).unwrap().is_empty());
        store.options.max_total = 4 * 1024 * 1024;
        let lifetime = store.reserve_lifetime(request(&store.root)).unwrap();
        let object = store.object(b"protected preparation bytes").unwrap();
        let config: std::sync::Arc<crate::ConfigFn> =
            std::sync::Arc::new(|_| serde_json::json!({"snapshots":{"retention_days":0}}));
        super::super::gc_location(&store.namespace, &config, &Default::default()).unwrap();
        assert!(storage::open(&store.objects, Path::new(&object.object)).is_ok());
        lifetime.verify().unwrap();
    }

    #[test]
    #[ignore = "subprocess fixture invoked by lifetime proof test"]
    fn lifetime_owner_child() {
        let root = PathBuf::from(std::env::var_os("CYBER_SNAPSHOT_LIFETIME_CHILD_ROOT").unwrap());
        let store = store(&root);
        if std::env::var_os("CYBER_SNAPSHOT_LIFETIME_CHILD_STAGE").is_some() {
            let result = store.reserve_lifetime_before_publish(request(&store.root), || {
                publish_ready(&root, b"stage");
                std::io::stdin().read_exact(&mut [0]).unwrap();
                Err("injected pre-publication cut".into())
            });
            assert!(result.is_err());
            return;
        }
        let lifetime = store.reserve_lifetime(request(&store.root)).unwrap();
        drop(store);
        publish_ready(&root, &serde_json::to_vec(lifetime.binding()).unwrap());
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte).unwrap();
        lifetime.verify().unwrap();
    }

    fn publish_ready(root: &Path, bytes: &[u8]) {
        std::fs::write(root.join("ready.tmp"), bytes).unwrap();
        std::fs::rename(root.join("ready.tmp"), root.join("ready.json")).unwrap();
    }

    #[test]
    fn real_process_death_before_publication_retains_stage_without_poisoning_the_owner_name() {
        use std::process::{Command, Stdio};
        let temp = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "copy::lifetime::tests::lifetime_owner_child",
                "--ignored",
                "--nocapture",
            ])
            .env("CYBER_SNAPSHOT_LIFETIME_CHILD_ROOT", temp.path())
            .env("CYBER_SNAPSHOT_LIFETIME_CHILD_STAGE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !temp.path().join("ready.json").exists() && std::time::Instant::now() < deadline {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("lifetime fixture exited: {status}");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(temp.path().join("ready.json").exists());
        child.kill().unwrap();
        child.wait().unwrap();
        let store = store(temp.path());
        let directory = storage::existing_child(&store.namespace, "workers", true).unwrap();
        let stages = storage::names(&directory, 10).unwrap();
        assert_eq!(stages.len(), 2);
        assert!(stages.iter().any(|name| name.starts_with("preparing_")));
        assert!(stages.iter().any(|name| name.starts_with("phase_")));
        let retained: Vec<_> = stages
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    storage::read(&directory, name, RECORD_LIMIT).unwrap(),
                )
            })
            .collect();
        let lifetime = store.reserve_lifetime(request(&store.root)).unwrap();
        for (name, bytes) in retained {
            assert_eq!(
                storage::read(&directory, &name, RECORD_LIMIT).unwrap(),
                bytes
            );
        }
        assert_eq!(storage::names(&directory, 10).unwrap().len(), 4);
        lifetime.verify().unwrap();
    }

    #[test]
    fn real_process_death_releases_only_the_exact_worker_lifetime_lock() {
        use std::process::{Command, Stdio};
        let temp = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "copy::lifetime::tests::lifetime_owner_child",
                "--ignored",
                "--nocapture",
            ])
            .env("CYBER_SNAPSHOT_LIFETIME_CHILD_ROOT", temp.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let ready = temp.path().join("ready.json");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready.exists() && std::time::Instant::now() < deadline {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("lifetime fixture exited: {status}");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(ready.exists());
        let binding: SnapshotLifetimeBinding =
            serde_json::from_slice(&std::fs::read(ready).unwrap()).unwrap();
        let store = store(temp.path());
        assert!(store.lock_lifetime(&binding).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
        let proof = store.lock_lifetime(&binding).unwrap();
        assert_eq!(proof.binding(), &binding);
        proof.verify().unwrap();
    }
}
