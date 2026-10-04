//! Backup bundles: the database plus artifacts, with a manifest of hashes
//! (`storage-events` → Backup; P0 exit criterion "a full database-plus-artifact backup
//! restores and verifies its manifest").

use std::path::{Path, PathBuf};

use cyber_core::paths::Paths;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MANIFEST: &str = "manifest.json";
const DATABASE: &str = "cyber.db";
/// Data directories copied with `--artifacts`, relative to `<data>`.
const ARTIFACTS: &[&str] = &["tool-output", "snapshot"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub kind: String,
    pub created_at: String,
    pub cyber_version: String,
    pub database: String,
    pub artifacts: Vec<String>,
    pub files: Vec<BundleFile>,
}

/// Write a bundle directory at `dest`: an online copy of the database, optionally the
/// artifact directories, and a manifest hashing every file.
pub fn create(
    paths: &Paths,
    db: &Path,
    dest: &Path,
    artifacts: bool,
) -> Result<BundleManifest, String> {
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let result = write_bundle(paths, db, dest, artifacts);
    if result.is_err() {
        // Never leave a half-written bundle that could be mistaken for a backup.
        let _ = std::fs::remove_dir_all(dest);
    }
    result
}

fn write_bundle(
    paths: &Paths,
    db: &Path,
    dest: &Path,
    artifacts: bool,
) -> Result<BundleManifest, String> {
    cyber_store::backup::backup(db, &dest.join(DATABASE)).map_err(|e| e.to_string())?;
    let mut included = Vec::new();
    if artifacts {
        for name in ARTIFACTS {
            let source = paths.data.join(name);
            if source.is_dir() {
                copy_tree(&source, &dest.join("artifacts").join(name))
                    .map_err(|e| format!("{name}: {e}"))?;
                included.push((*name).to_string());
            }
        }
    }
    let manifest = BundleManifest {
        schema_version: 1,
        kind: "cyber-backup".into(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        cyber_version: crate::version().into(),
        database: DATABASE.into(),
        artifacts: included,
        files: hash_files(dest).map_err(|e| e.to_string())?,
    };
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(dest.join(MANIFEST), text + "\n").map_err(|e| e.to_string())?;
    Ok(manifest)
}

/// Check every manifest entry (size and hash), unexpected files, and database integrity.
pub fn verify(dir: &Path) -> Result<BundleManifest, Vec<String>> {
    let text = std::fs::read_to_string(dir.join(MANIFEST))
        .map_err(|e| vec![format!("{MANIFEST}: {e}")])?;
    let manifest: BundleManifest =
        serde_json::from_str(&text).map_err(|e| vec![format!("{MANIFEST}: {e}")])?;
    let actual = hash_files(dir).map_err(|e| vec![e.to_string()])?;
    let mut problems = Vec::new();
    for expected in &manifest.files {
        match actual.iter().find(|f| f.path == expected.path) {
            None => problems.push(format!("missing: {}", expected.path)),
            Some(f) if f != expected => problems.push(format!("changed: {}", expected.path)),
            Some(_) => {}
        }
    }
    for extra in actual
        .iter()
        .filter(|f| !manifest.files.iter().any(|e| e.path == f.path))
    {
        problems.push(format!("unexpected: {}", extra.path));
    }
    match cyber_store::backup::integrity_check(&dir.join(&manifest.database)) {
        Ok(result) if result == "ok" => {}
        Ok(result) => problems.push(format!("database integrity: {result}")),
        Err(e) => problems.push(format!("database: {e}")),
    }
    if problems.is_empty() {
        Ok(manifest)
    } else {
        Err(problems)
    }
}

/// Replace the database (and artifacts, for bundles) from a verified backup. The current
/// files are kept beside the originals as `*.pre-restore-<id>`. Refuses while a server is
/// registered.
pub fn restore(paths: &Paths, db: &Path, source: &Path) -> Result<Vec<PathBuf>, String> {
    if let Some(reg) = crate::read_registration(paths) {
        return Err(format!(
            "a server is registered (pid {}); run `cyber service stop` first",
            reg.pid
        ));
    }
    let suffix = format!(
        "pre-restore-{}",
        ulid::Ulid::new().to_string().to_lowercase()
    );
    let (database, artifacts) = if source.is_dir() {
        let manifest = verify(source)
            .map_err(|p| format!("the bundle failed verification:\n  {}", p.join("\n  ")))?;
        (
            source.join(&manifest.database),
            manifest
                .artifacts
                .iter()
                .map(|a| (source.join("artifacts").join(a), paths.data.join(a)))
                .collect(),
        )
    } else {
        match cyber_store::backup::integrity_check(source) {
            Ok(r) if r == "ok" => (source.to_path_buf(), Vec::new()),
            Ok(r) => return Err(format!("{}: integrity check failed: {r}", source.display())),
            Err(e) => return Err(format!("{}: {e}", source.display())),
        }
    };
    let mut kept = Vec::new();
    for sidecar in ["", "-wal", "-shm"] {
        let current = PathBuf::from(format!("{}{sidecar}", db.display()));
        if current.exists() {
            let aside = PathBuf::from(format!("{}.{suffix}", current.display()));
            std::fs::rename(&current, &aside).map_err(|e| e.to_string())?;
            kept.push(aside);
        }
    }
    std::fs::copy(&database, db).map_err(|e| e.to_string())?;
    for (from, to) in artifacts {
        if to.exists() {
            let aside = to.with_file_name(format!(
                "{}.{suffix}",
                to.file_name().unwrap_or_default().to_string_lossy()
            ));
            std::fs::rename(&to, &aside).map_err(|e| e.to_string())?;
            kept.push(aside);
        }
        copy_tree(&from, &to).map_err(|e| e.to_string())?;
    }
    Ok(kept)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Every file under `dir` except the manifest, sorted, with size and SHA-256.
fn hash_files(dir: &Path) -> std::io::Result<Vec<BundleFile>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(dir)
                .map_err(std::io::Error::other)?
                .to_string_lossy()
                .replace('\\', "/");
            if rel == MANIFEST {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            out.push(BundleFile {
                path: rel,
                size: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}
