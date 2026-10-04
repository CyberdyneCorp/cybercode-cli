//! Test helper for process-kill recovery: appends events forever and prints `acked <seq>`
//! after each commit. The integration test kills it with SIGKILL and verifies the store.

use std::error::Error;
use std::io::Write;

use cyber_core::paths::DatabaseLocation;
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreOptions};
use serde_json::json;

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: crash_writer <db path>")?;
    let mut registry = EventRegistry::default();
    registry.register("test.counter.incremented.1")?;
    let store = Store::open(StoreOptions::new(
        DatabaseLocation::File(path.into()),
        registry,
    ))?;
    let mut out = std::io::stdout().lock();
    for i in 0_u64.. {
        let event = NewEvent::new("test.counter.incremented.1", json!({ "i": i }));
        let stored = store.append("agg_crash", Expected::Any, vec![event])?;
        writeln!(out, "acked {}", stored[0].seq)?;
        out.flush()?;
    }
    Ok(())
}
