//! Application-owned Store authority for private snapshot maintenance.
use cyber_server::runtime::{
    SnapshotOrigin,
    snapshot_reclamation::{Plan, Receipt, Record},
};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct Settlement {
    pub receipt: Receipt,
    pub settled_at_ms: i64,
}

pub type Delete = Box<dyn FnOnce() -> Result<u64, String> + Send>;
pub type Collect = Box<dyn FnOnce(&BTreeSet<String>) -> Result<(), String> + Send>;
/// Implementations serialize verification/deletion against reference writers.
/// A durable plan must be committed before invoking `delete`; native guards are
/// retained by that closure. Maintenance never acknowledges a native activity.
pub trait Policy: Send + Sync {
    fn receipt(&self, origin: &SnapshotOrigin) -> Result<Option<Settlement>, String>;
    fn record(&self, receipt: &Receipt) -> Result<Option<Record>, String>;
    fn reclaim(&self, plan: Plan, delete: Delete) -> Result<u64, String>;
    /// Retire exact metadata only after payload completion. The callback must
    /// retain native ownership proof; the default never invokes it.
    fn retire(
        &self,
        _payload: Plan,
        _plan: cyber_server::runtime::snapshot_reclamation::metadata::Plan,
        _delete: Delete,
    ) -> Result<u64, String> {
        Err("Snapshot metadata retirement is unavailable".into())
    }
    fn collect(&self, collect: Collect) -> Result<(), String>;
    fn refused(&self, directory: &str, error: &str);
}
