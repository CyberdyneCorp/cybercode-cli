//! Durable policy provenance, published before the first manifest unlink.
use super::{Manifest, read_manifest, storage};
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, path::Path};
use storage::{Result, fail};

const FILE: &str = "collection.json";
const LIMIT: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    location: std::path::PathBuf,
    root: String,
    namespace: String,
}
impl Record {
    fn from_manifest(dir: &Dir, manifest: Manifest) -> Result<Self> {
        Ok(Self {
            version: 1,
            location: manifest.tree.location,
            root: manifest.tree.root,
            namespace: storage::dir_identity(dir)?,
        })
    }
    fn validate(&self, dir: &Dir, namespace: &str) -> Result<()> {
        let key = storage::digest(format!("{}\0{}", self.location.display(), self.root).as_bytes());
        if self.version != 1
            || !self.location.is_absolute()
            || self.location.parent().is_none()
            || !super::super::ownership::valid_identity(&self.root)
            || self.namespace != storage::dir_identity(dir)?
            || key != namespace
        {
            return Err("Corrupt or foreign snapshot collection provenance".into());
        }
        Ok(())
    }
}

struct Pin {
    file: File,
    identity: String,
    generation: String,
}
impl Pin {
    fn new(file: File) -> Result<Self> {
        storage::validate_lock(&file)?;
        Ok(Self {
            identity: storage::identity(&file)?,
            generation: super::super::ownership::generation(&file)?,
            file,
        })
    }
    fn verify(&self, dir: &Dir) -> Result<()> {
        let route = storage::open(dir, Path::new(FILE))?;
        storage::validate_lock(&route)?;
        if storage::identity(&route)? != self.identity
            || super::super::ownership::generation(&route)? != self.generation
            || super::super::ownership::generation(&self.file)? != self.generation
        {
            return Err("Snapshot collection provenance identity or generation changed".into());
        }
        Ok(())
    }
}

pub(super) struct Provenance {
    record: Record,
    directory: Dir,
    pin: Option<Pin>,
}
impl Provenance {
    pub fn inspect(dir: &Dir, manifests: &Dir, names: &[String], namespace: &str) -> Result<Self> {
        let retained = match dir.symlink_metadata(FILE) {
            Ok(_) => Some(Self::load(dir, namespace)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(fail(error)),
        };
        let mut record = retained.as_ref().map(|(record, _)| record.clone());
        // Preflight every surviving manifest, including after a prior collection.
        for name in names {
            if super::temporary(name) {
                storage::open(manifests, Path::new(name))?;
                continue;
            }
            let observed = Record::from_manifest(dir, read_manifest(manifests, name, namespace)?)?;
            observed.validate(dir, namespace)?;
            if record.as_ref().is_some_and(|record| *record != observed) {
                return Err("Snapshot collection has inconsistent Location evidence".into());
            }
            record = Some(observed);
        }
        Ok(Self {
            record: record.ok_or("Snapshot collection lacks Location policy evidence")?,
            directory: dir.try_clone().map_err(fail)?,
            pin: retained.map(|(_, pin)| pin),
        })
    }
    fn load(dir: &Dir, namespace: &str) -> Result<(Record, Pin)> {
        let pin = Pin::new(storage::open(dir, Path::new(FILE))?)?;
        if pin.file.metadata().map_err(fail)?.len() > LIMIT {
            return Err("Snapshot collection provenance exceeds limit".into());
        }
        let mut bytes = Vec::new();
        (&pin.file)
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(fail)?;
        if bytes.len() as u64 > LIMIT {
            return Err("Snapshot collection provenance grew beyond limit".into());
        }
        pin.verify(dir)?;
        let record: Record = serde_json::from_slice(&bytes).map_err(fail)?;
        record.validate(dir, namespace)?;
        Ok((record, pin))
    }
    pub fn location(&self) -> &Path {
        &self.record.location
    }
    pub fn publish(&mut self) -> Result<()> {
        if self.pin.is_some() {
            return self.verify();
        }
        let bytes = serde_json::to_vec(&self.record).map_err(fail)?;
        if bytes.len() as u64 > LIMIT {
            return Err("Snapshot collection provenance exceeds limit".into());
        }
        let temporary = format!("tmp_{}", ulid::Ulid::new());
        let file = storage::create(&self.directory, &temporary, &bytes)?;
        rustix::fs::renameat_with(
            &self.directory,
            &temporary,
            &self.directory,
            FILE,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(fail)?;
        storage::sync(&self.directory)?;
        self.pin = Some(Pin::new(file)?);
        self.verify()
    }
    pub fn verify(&self) -> Result<()> {
        self.pin
            .as_ref()
            .ok_or("Snapshot collection provenance was not published")?
            .verify(&self.directory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copy::{Options, Store, collection::gc_location};
    use std::collections::BTreeSet;

    fn reopen(base: &Path) -> Store {
        Store::recover_existing(
            &base.join("data"),
            base.join("work").to_str().unwrap(),
            options(),
        )
        .unwrap()
    }
    fn options() -> Options {
        Options {
            max_file: 65536,
            max_total: 4 * 1024 * 1024,
            ignore: vec![],
            private_roots: vec![],
        }
    }
    fn key(store: &Store) -> String {
        storage::digest(format!("{}\0{}", store.root.display(), store.root_id).as_bytes())
    }
    fn fixture() -> (tempfile::TempDir, Store) {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        std::fs::create_dir(&work).unwrap();
        std::fs::write(work.join("owner"), b"owner bytes").unwrap();
        let store =
            Store::open(&temp.path().join("data"), work.to_str().unwrap(), options()).unwrap();
        store.track().unwrap();
        for name in storage::names(&store.manifests, 10).unwrap() {
            let mut manifest: Manifest = serde_json::from_slice(
                &storage::read(&store.manifests, &name, super::super::RECORD_LIMIT).unwrap(),
            )
            .unwrap();
            manifest.accessed = 0;
            storage::atomic(
                &store.manifests,
                &name,
                &serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
        }
        for name in storage::names(&store.objects, 10).unwrap() {
            storage::open(&store.objects, Path::new(&name))
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
                .unwrap();
        }
        (temp, store)
    }
    fn publish(store: &Store) -> Provenance {
        let names = storage::names(&store.manifests, 10).unwrap();
        let mut proof =
            Provenance::inspect(&store.namespace, &store.manifests, &names, &key(store)).unwrap();
        proof.publish().unwrap();
        proof
    }
    fn collect(store: &Store, config: &crate::ConfigFn) -> Result<()> {
        gc_location(&store.namespace, &key(store), config, &BTreeSet::new())
    }

    #[test]
    fn a_foreign_or_malformed_record_cannot_be_overwritten_from_manifest_evidence() {
        let (_temp, store) = fixture();
        let (_foreign_temp, foreign) = fixture();
        let foreign_proof = publish(&foreign);
        storage::create(
            &store.namespace,
            FILE,
            &serde_json::to_vec(&foreign_proof.record).unwrap(),
        )
        .unwrap();
        for invalid in [false, true] {
            if invalid {
                store.namespace.write(FILE, b"{").unwrap();
            }
            let bytes = storage::read(&store.namespace, FILE, LIMIT).unwrap();
            assert!(collect(&store, &|_| serde_json::json!({})).is_err());
            assert_eq!(storage::read(&store.namespace, FILE, LIMIT).unwrap(), bytes);
            assert_eq!(storage::names(&store.manifests, 10).unwrap().len(), 1);
            assert_eq!(storage::names(&store.objects, 10).unwrap().len(), 1);
        }
    }

    #[test]
    fn metadata_replacement_after_preflight_refuses_before_unlinks() {
        for replace in [false, true] {
            changed_metadata_refuses(replace);
        }
    }
    fn changed_metadata_refuses(replace: bool) {
        let (_temp, store) = fixture();
        let proof = publish(&store);
        let bytes = serde_json::to_vec(&proof.record).unwrap();
        let called = std::sync::atomic::AtomicUsize::new(0);
        let directory = store.namespace.try_clone().unwrap();
        let result = collect(&store, &move |_| {
            if called.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                if replace {
                    directory
                        .rename(FILE, &directory, "retained-record")
                        .unwrap();
                    storage::create(&directory, FILE, &bytes).unwrap();
                } else {
                    let mut changed = bytes.clone();
                    changed.push(b'\n');
                    directory.write(FILE, changed).unwrap();
                }
            }
            serde_json::json!({})
        });
        assert!(
            result
                .unwrap_err()
                .contains("identity or generation changed")
        );
        assert_eq!(storage::names(&store.manifests, 10).unwrap().len(), 1);
        assert_eq!(storage::names(&store.objects, 10).unwrap().len(), 1);
    }

    #[test]
    fn provenance_cannot_be_transplanted_into_another_native_namespace() {
        let (temp, store) = fixture();
        let proof = publish(&store);
        let other = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        let other = storage::private_child(&other, "foreign-namespace").unwrap();
        storage::create(&other, FILE, &serde_json::to_vec(&proof.record).unwrap()).unwrap();
        assert!(Provenance::load(&other, &key(&store)).is_err());
        assert_eq!(storage::names(&store.manifests, 10).unwrap().len(), 1);
    }

    #[test]
    fn provenance_aliases_never_authorize_collection() {
        for symbolic in [false, true] {
            let (_temp, store) = fixture();
            let proof = publish(&store);
            store
                .namespace
                .rename(FILE, &store.namespace, "retained-record")
                .unwrap();
            if symbolic {
                rustix::fs::symlinkat("retained-record", &store.namespace, FILE).unwrap();
            } else {
                store
                    .namespace
                    .hard_link("retained-record", &store.namespace, FILE)
                    .unwrap();
            }
            assert!(collect(&store, &|_| serde_json::json!({})).is_err());
            assert_eq!(storage::names(&store.manifests, 10).unwrap().len(), 1);
            assert_eq!(storage::names(&store.objects, 10).unwrap().len(), 1);
            let retained: Record =
                serde_json::from_slice(&store.namespace.read("retained-record").unwrap()).unwrap();
            assert_eq!(retained, proof.record);
        }
    }

    #[test]
    fn namespace_metadata_and_unpublished_construction_count_toward_quota() {
        let (temp, store) = fixture();
        let before = store.namespace_metadata_size().unwrap();
        let proof = publish(&store);
        let record_size = proof.pin.as_ref().unwrap().file.metadata().unwrap().len();
        storage::create(&store.namespace, "tmp_unpublished", &[0; 2048]).unwrap();
        assert_eq!(
            store.namespace_metadata_size().unwrap(),
            before + record_size + 2048
        );
        drop(proof);
        drop(store);
        let reopened = reopen(temp.path());
        assert!(reopened.usage().unwrap() >= record_size + 2048);
    }

    #[test]
    #[ignore = "subprocess fixture invoked by collection-restart proof"]
    fn collection_child() {
        let base = std::path::PathBuf::from(std::env::var_os("CYBER_COLLECTION_CHILD").unwrap());
        let store = reopen(&base);
        let manifests = store.manifests.try_clone().unwrap();
        collect(&store, &move |_| {
            if storage::names(&manifests, 10).unwrap().is_empty() {
                std::fs::write(base.join("ready"), b"last manifest gone, objects retained")
                    .unwrap();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
            serde_json::json!({})
        })
        .unwrap();
        panic!("child passed the cut");
    }

    #[test]
    fn process_death_after_last_manifest_unlink_resumes_from_durable_policy_provenance() {
        use std::os::unix::process::ExitStatusExt;
        let (temp, original) = fixture();
        drop(original);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "copy::collection::provenance::tests::collection_child",
                "--ignored",
                "--nocapture",
            ])
            .env("CYBER_COLLECTION_CHILD", temp.path())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let ready = loop {
            if temp.path().join("ready").exists() {
                break true;
            }
            if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let _ = child.kill();
        let status = child.wait().unwrap();
        assert!(ready, "child missed the collection cut: {status}");
        assert_eq!(status.signal(), Some(9));
        let reopened = reopen(temp.path());
        assert!(storage::names(&reopened.manifests, 10).unwrap().is_empty());
        assert_eq!(storage::names(&reopened.objects, 10).unwrap().len(), 1);
        reopened
            .namespace
            .rename(FILE, &reopened.namespace, "retained-record")
            .unwrap();
        assert!(collect(&reopened, &|_| serde_json::json!({})).is_err());
        assert_eq!(storage::names(&reopened.objects, 10).unwrap().len(), 1);
        reopened
            .namespace
            .rename("retained-record", &reopened.namespace, FILE)
            .unwrap();
        assert_preserved_by_unavailable_and_disabled_policy(&reopened);
        collect(&reopened, &|_| serde_json::json!({})).unwrap();
        assert!(storage::names(&reopened.objects, 10).unwrap().is_empty());
        assert!(reopened.namespace.symlink_metadata(FILE).unwrap().is_file());
        assert_eq!(
            std::fs::read(temp.path().join("work/owner")).unwrap(),
            b"owner bytes"
        );
    }
    fn assert_preserved_by_unavailable_and_disabled_policy(store: &Store) {
        assert!(
            collect(store, &|_| serde_json::Value::Null)
                .unwrap_err()
                .contains("configuration is unavailable")
        );
        assert_eq!(storage::names(&store.objects, 10).unwrap().len(), 1);
        for settings in [
            serde_json::json!({"snapshots":false}),
            serde_json::json!({"snapshots":{"retention_days":0}}),
        ] {
            collect(store, &move |_| settings.clone()).unwrap();
            assert_eq!(storage::names(&store.objects, 10).unwrap().len(), 1);
        }
    }
}
