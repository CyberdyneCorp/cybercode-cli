//! Debug-build process-death fixture points; see `cyber_core::process_cut`.

/// Block the calling thread forever at `point` when the fixture selects it.
pub(crate) fn pause(point: &str) {
    cyber_core::process_cut::pause(point);
}
