//! Real Store/native maintenance and a plan-only wrapper for interruption tests.
use cyber_server::runtime::{
    SnapshotOrigin,
    snapshot_reclamation::{Plan, Receipt, Record},
};
use cyber_snapshot::reclamation::{Collect, Delete, Policy, Settlement};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub struct Planned(pub Arc<dyn Policy>, pub Arc<Mutex<Option<PathBuf>>>);
impl Policy for Planned {
    fn receipt(&self, origin: &SnapshotOrigin) -> Result<Option<Settlement>, String> {
        self.0.receipt(origin)
    }
    fn record(&self, receipt: &Receipt) -> Result<Option<Record>, String> {
        self.0.record(receipt)
    }
    fn reclaim(&self, plan: Plan, delete: Delete) -> Result<u64, String> {
        let injection = self.1.lock().unwrap().clone();
        self.0.reclaim(
            plan,
            Box::new(move || {
                if let Some(path) = injection {
                    std::fs::write(path, b"unplanned owner evidence").unwrap();
                    return delete();
                }
                let _native_guards = delete;
                Err("fixture cut after durable paired plan".into())
            }),
        )
    }
    fn collect(&self, collect: Collect) -> Result<(), String> {
        self.0.collect(collect)
    }
    fn refused(&self, directory: &str, error: &str) {
        self.0.refused(directory, error);
    }
}

pub struct Maintenance {
    pub store: Arc<cyber_store::Store>,
    pub policy: Arc<dyn Policy>,
    pub namespace: PathBuf,
    root: PathBuf,
    runtime: tokio::runtime::Runtime,
    snapshots: Arc<cyber_snapshot::GitSnapshots>,
}
impl Maintenance {
    pub fn open(root: &Path) -> Self {
        let data = root.join("data");
        let database = std::fs::read_dir(&data)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|extension| extension == "db"))
            .unwrap();
        let store = Arc::new(
            cyber_store::Store::open(cyber_store::StoreOptions::new(
                cyber_core::paths::DatabaseLocation::File(database),
                cyber_server::runtime::Runtime::registry(),
            ))
            .unwrap(),
        );
        let policy = cyber_app::snapshot_reclamation::policy(store.clone());
        let namespace = std::fs::read_dir(data.join("snapshot/global"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let snapshots = Self::snapshots(root, policy.clone());
        Self {
            store,
            policy,
            namespace,
            root: root.into(),
            runtime: tokio::runtime::Runtime::new().unwrap(),
            snapshots,
        }
    }
    fn snapshots(root: &Path, policy: Arc<dyn Policy>) -> Arc<cyber_snapshot::GitSnapshots> {
        Self::configured(
            root,
            policy,
            Arc::new(|_| serde_json::json!({"snapshots":{"retention_days":7}})),
        )
    }
    fn configured(
        root: &Path,
        policy: Arc<dyn Policy>,
        config: Arc<cyber_snapshot::ConfigFn>,
    ) -> Arc<cyber_snapshot::GitSnapshots> {
        cyber_snapshot::GitSnapshots::with_reclamation(
            root.join("data"),
            config,
            vec![root.join("config"), root.join("state"), root.join("cache")],
            None,
            Some(policy),
        )
    }
    pub fn reclaim(&self, source: &Path) -> Result<u64, String> {
        self.runtime
            .block_on(self.snapshots.reclaim(source.to_str().unwrap()))
    }
    pub fn retire(&self, source: &Path, receipt: Receipt) -> Result<u64, String> {
        self.runtime
            .block_on(self.snapshots.retire(source.to_str().unwrap(), receipt))
    }
    pub fn plan_only(&self, source: &Path) -> Result<u64, String> {
        let cut = Self::snapshots(
            &self.root,
            Arc::new(Planned(self.policy.clone(), Arc::new(Mutex::new(None)))),
        );
        self.runtime.block_on(cut.reclaim(source.to_str().unwrap()))
    }
    /// Run real reclamation until `first` has been unlinked, then block this process
    /// forever. The next per-payload retention check runs inside the open deletion
    /// transaction, after the durable plan and before its completion commit.
    pub fn reclaim_until_killed(&self, source: &Path, first: PathBuf, ready: PathBuf) -> String {
        let config = Arc::new(move |_: &Path| {
            if std::fs::symlink_metadata(&first).is_ok() {
                return serde_json::json!({"snapshots":{"retention_days":7}});
            }
            std::fs::write(&ready, b"payload deletion in progress").unwrap();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        });
        let cut = Self::configured(&self.root, self.policy.clone(), config);
        let result = self.runtime.block_on(cut.reclaim(source.to_str().unwrap()));
        format!("{result:?}")
    }
    pub fn age(&self, activity: String) {
        self.store
            .transaction(move |db| {
                db.execute(
                    "UPDATE event SET time=time-?1 WHERE aggregate_id=?2",
                    rusqlite::params![8i64 * 86400000, activity],
                )?;
                Ok(())
            })
            .unwrap();
    }
}
