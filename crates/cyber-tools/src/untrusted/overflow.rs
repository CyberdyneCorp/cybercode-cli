//! Provenance of managed overflow files (`untrusted-content` → Framed tool results).
//!
//! When a framed result overflows, its full text goes to a managed file and its
//! provenance to a `<file>.provenance.json` sidecar. A later `read` or `grep` of
//! anything in the managed output directory is framed again: with the recorded
//! provenance when the sidecar is intact, and otherwise, failing closed, with
//! `source="overflow"` and the file path as `origin`. Re-reading never strips a frame.

use std::path::{Path, PathBuf};

use cyber_server::runtime::{ContentSource, Provenance};

const SIDECAR_SUFFIX: &str = ".provenance.json";

fn sidecar(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(SIDECAR_SUFFIX);
    PathBuf::from(name)
}

/// Record `provenance` beside the overflow file at `path`.
pub(crate) fn record(path: &Path, provenance: &Provenance) -> Result<(), String> {
    let json = serde_json::to_vec(provenance)
        .map_err(|e| format!("could not record the output's provenance: {e}"))?;
    std::fs::write(sidecar(path), json)
        .map_err(|e| format!("could not record the output's provenance: {e}"))
}

/// The provenance recorded for `path`, or the `overflow` fallback when the record is
/// missing or unreadable.
pub(crate) fn recorded(path: &Path) -> Provenance {
    std::fs::read(sidecar(path))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Provenance>(&bytes).ok())
        .unwrap_or_else(|| Provenance::new(ContentSource::Overflow, path.display().to_string()))
}

/// Whether `path` lies in the managed output directory `dir`. Both sides are
/// canonicalized when they exist, so links and `/private` aliases still match.
pub(crate) fn managed(dir: &Path, path: &Path) -> bool {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    path.starts_with(dir) || real(path).starts_with(real(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tool_1");
        std::fs::write(&file, "text").unwrap();
        let fallback = Provenance::new(ContentSource::Overflow, file.display().to_string());
        assert_eq!(recorded(&file), fallback);
        let web = Provenance::new(ContentSource::Webfetch, "https://x.example/a");
        record(&file, &web).unwrap();
        assert_eq!(recorded(&file), web);
        std::fs::write(sidecar(&file), "{not json").unwrap();
        assert_eq!(recorded(&file), fallback);
        std::fs::write(sidecar(&file), r#"{"source":"system","origin":"x"}"#).unwrap();
        assert_eq!(recorded(&file), fallback);
    }

    #[test]
    fn managed_paths_are_inside_the_output_directory() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("tool-output");
        std::fs::create_dir(&output).unwrap();
        assert!(managed(&output, &output.join("tool_1")));
        assert!(managed(&output, &output));
        assert!(!managed(&output, &dir.path().join("src/main.rs")));
        assert!(!managed(&output, &dir.path().join("tool-output-2/x")));
    }
}
