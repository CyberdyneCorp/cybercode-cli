//! Stop after a durable plan; native payload deletion is deliberately not run.
use cyber_server::runtime::{
    SnapshotOrigin,
    snapshot_reclamation::{Plan, Receipt, Record},
};
use cyber_snapshot::reclamation::{Collect, Delete, Policy, Settlement};
use std::{
    path::PathBuf,
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
