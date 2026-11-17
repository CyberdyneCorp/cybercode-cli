//! Debug-build process-death fixture points for crash-recovery tests.
//!
//! Some recovery boundaries have no externally observable pause, such as the
//! window between a flushed private construction stage and its no-replace
//! publication. Real-binary tests start the service with
//! `CYBER_TEST_PROCESS_CUT=<point>` and `CYBER_TEST_PROCESS_CUT_MARKER=<file>`,
//! wait for the marker and then kill the process. Release builds ignore both
//! variables; the call compiles to nothing.

/// Block the calling thread forever at `point` when the fixture selects it.
pub fn pause(point: &str) {
    #[cfg(debug_assertions)]
    if std::env::var_os("CYBER_TEST_PROCESS_CUT").is_some_and(|selected| selected == point) {
        if let Some(marker) = std::env::var_os("CYBER_TEST_PROCESS_CUT_MARKER") {
            // Best effort: the test times out if the marker cannot be written.
            let _ = std::fs::write(marker, point);
        }
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = point;
}
