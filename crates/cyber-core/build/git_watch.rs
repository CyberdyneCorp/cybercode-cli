//! The git files `build.rs` watches so the build identity follows the commit. Shared with
//! `tests/build_script.rs`.
use std::path::{Path, PathBuf};

/// Of `HEAD`, `packed-refs` and the branch ref (`refs/...`), the paths cargo should watch,
/// given each one's name and path. Cargo reruns a build script on every build when a
/// watched path does not exist, which rebuilt every crate above `cyber-core` each time in
/// a checkout without `packed-refs` or a branch (CI). So a missing `HEAD` or `packed-refs`
/// is left out, and a branch ref that is only packed is watched through its nearest
/// existing directory, where the next commit writes the loose ref.
pub fn watched(refs: &[(String, PathBuf)]) -> Vec<PathBuf> {
    refs.iter()
        .filter_map(|(name, path)| {
            if path.exists() {
                Some(path.clone())
            } else if name.starts_with("refs/") {
                path.ancestors()
                    .skip(1)
                    .find(|dir| dir.is_dir())
                    .map(Path::to_path_buf)
            } else {
                None
            }
        })
        .collect()
}
