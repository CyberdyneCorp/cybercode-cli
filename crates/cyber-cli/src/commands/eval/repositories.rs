//! Host-only, hash-checked source import. No repository setup code is executed here.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use clap::Args;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::error::CliError;

const MAX_ARCHIVE: usize = 32 * 1024 * 1024;
const MAX_SOURCE: u64 = 128 * 1024 * 1024;

#[derive(Debug, Args)]
pub struct PrepareArgs {
    pub catalog: PathBuf,
    /// New directory; existing destinations are refused.
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema_version: u32,
    id: String,
    description: String,
    tasks: Vec<Recipe>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    id: String,
    repository: String,
    base_commit: String,
    reference_commit: String,
    archive_sha256: String,
    tree_sha256: String,
    license: String,
    lock: Vec<String>,
    prompt: String,
    checks: Vec<Vec<String>>,
    grader_dir: String,
}

pub fn prepare(args: PrepareArgs) -> Result<(), CliError> {
    super::super::serve::runtime()?
        .block_on(import(&args))
        .map_err(CliError::usage)
}

async fn import(args: &PrepareArgs) -> Result<(), String> {
    let path = args.catalog.canonicalize().map_err(|e| e.to_string())?;
    let source_root = super::repo_root(&path).map_err(|e| e.message)?;
    let catalog: Catalog =
        serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    validate_catalog(&catalog)?;
    if args.output.exists() {
        return Err("evaluation output already exists".into());
    }
    let parent = args
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stage = tempfile::tempdir_in(parent).map_err(|e| e.to_string())?;
    let root = stage.path();
    let http = reqwest::Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let mut tasks = Vec::new();
    for recipe in &catalog.tasks {
        let url = format!(
            "https://codeload.github.com/{}/tar.gz/{}",
            recipe.repository, recipe.base_commit
        );
        let response = http
            .get(url)
            .send()
            .await
            .map_err(|_| "source download failed")?
            .error_for_status()
            .map_err(|_| "source download returned an error")?;
        let mut stream = response.bytes_stream();
        let mut archive = Vec::new();
        while let Some(bytes) = stream.next().await {
            let bytes = bytes.map_err(|_| "source body interrupted")?;
            if bytes.len() > MAX_ARCHIVE.saturating_sub(archive.len()) {
                return Err("source archive too large".into());
            }
            archive.extend_from_slice(&bytes);
        }
        let fixture = format!("eval/fixtures/{}", recipe.id);
        let destination = root.join(&fixture);
        let revision = unpack(&archive, &destination, &recipe.archive_sha256)?;
        if revision.as_deref() != Some(recipe.base_commit.as_str()) {
            return Err("archive revision metadata does not match the pinned base".into());
        }
        let actual = cyber_core::eval::tree_sha256(&destination).map_err(|e| e.to_string())?;
        if actual != recipe.tree_sha256 {
            return Err(format!("{}: source tree mismatch", recipe.id));
        }
        super::trial::copy_dir(
            &source_root.join(&recipe.grader_dir),
            &root.join(&recipe.grader_dir),
        )
        .map_err(|e| e.to_string())?;
        let grader_hash = cyber_core::eval::tree_sha256(&root.join(&recipe.grader_dir))
            .map_err(|e| e.to_string())?;
        tasks.push(json!({
            "id":recipe.id,"prompt":recipe.prompt,"tags":["real-repository","retrospective"],
            "fixture":{"path":fixture,"tree_sha256":actual,"environment":{"lock":recipe.lock},
                "repository":{"url":format!("https://github.com/{}",recipe.repository),"base_commit":recipe.base_commit,
                "reference_commit":recipe.reference_commit,"archive_sha256":recipe.archive_sha256,"license":recipe.license}},
            "budget":{"max_tokens":1_500_000,"max_turns":60,"max_wall_seconds":1800},
            "timeout_seconds":1800,"checks":recipe.checks,
            "grading":{"grader_dir":recipe.grader_dir,"tree_sha256":grader_hash,"command":["python3","{grader_dir}/grade.py","{workspace}"]}
        }));
    }
    let manifest = json!({"schema_version":1,"id":catalog.id,"description":catalog.description,
        "kind":"suite","harness":{"min_version":"0.1.0","tools":"tools.v1"},"tasks":tasks});
    let parsed: cyber_core::eval::Manifest =
        serde_json::from_value(manifest.clone()).map_err(|e| e.to_string())?;
    let problems = cyber_core::eval::validate(&parsed, root);
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let manifests = root.join("eval/manifests");
    std::fs::create_dir_all(&manifests).map_err(|e| e.to_string())?;
    std::fs::write(
        manifests.join(format!("{}.json", catalog.id)),
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    // Keep staging cleanup armed until the complete, validated tree has moved.
    std::fs::rename(root, &args.output).map_err(|e| e.to_string())?;
    println!("prepared {}", args.output.display());
    Ok(())
}

fn validate_catalog(catalog: &Catalog) -> Result<(), String> {
    fn identifier(s: &str) -> bool {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }
    fn sha(s: &str, n: usize) -> bool {
        s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit())
    }
    let mut seen = HashSet::new();
    if catalog.schema_version != 1 || !identifier(&catalog.id) || catalog.tasks.is_empty() {
        return Err("invalid catalog header".into());
    }
    for task in &catalog.tasks {
        let parts: Vec<_> = task.repository.split('/').collect();
        let expected_grader = format!("eval/graders/{}", task.id);
        if !identifier(&task.id)
            || !seen.insert(&task.id)
            || parts.len() != 2
            || parts.iter().any(|p| !identifier(p))
            || !sha(&task.base_commit, 40)
            || !sha(&task.reference_commit, 40)
            || !sha(&task.archive_sha256, 64)
            || task.grader_dir != expected_grader
            || !super::verification::relative(Path::new(&task.grader_dir))
            || task.license.is_empty()
            || task.checks.is_empty()
            || task.checks.iter().any(|a| a.is_empty() || a[0].is_empty())
        {
            return Err(format!("invalid task source or checks: {}", task.id));
        }
    }
    Ok(())
}

fn unpack(bytes: &[u8], destination: &Path, expected: &str) -> Result<Option<String>, String> {
    if bytes.len() > MAX_ARCHIVE || format!("{:x}", Sha256::digest(bytes)) != expected {
        return Err("source archive digest mismatch".into());
    }
    std::fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    let reader = flate2::read::GzDecoder::new(bytes).take(MAX_SOURCE * 2);
    let mut archive = tar::Archive::new(reader);
    let mut seen = HashSet::new();
    let mut prefix = None;
    let mut remaining = MAX_SOURCE;
    let mut links = Vec::new();
    let mut revision = None;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if entry.header().entry_type().is_pax_global_extensions() {
            if entry.size() > 64 * 1024 {
                return Err("archive metadata too large".into());
            }
            for field in entry
                .pax_extensions()
                .map_err(|e| e.to_string())?
                .ok_or("missing global metadata")?
            {
                let field = field.map_err(|e| e.to_string())?;
                let value = std::str::from_utf8(field.value_bytes())
                    .map_err(|_| "invalid archive metadata")?;
                if field.key_bytes() != b"comment"
                    || revision.is_some()
                    || value.len() != 40
                    || !value.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err("unsupported global archive metadata".into());
                }
                revision = Some(value.to_owned());
            }
            continue;
        }
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let mut components = path.components();
        let first = components.next().ok_or("empty archive path")?;
        if !matches!(first, Component::Normal(_)) {
            return Err("unsafe archive prefix".into());
        }
        if prefix.get_or_insert(first.as_os_str().to_owned()) != first.as_os_str() {
            return Err("multiple archive roots".into());
        }
        let rest: PathBuf = components.collect();
        if rest
            .components()
            .any(|c| !matches!(c,Component::Normal(n) if !n.to_string_lossy().eq_ignore_ascii_case(".git") && !n.to_string_lossy().contains(['\\',':'])))
        {
            return Err("unsafe archive path".into());
        }
        if rest.as_os_str().is_empty() {
            if !entry.header().entry_type().is_dir() {
                return Err("archive root is not a directory".into());
            }
            continue;
        }
        if !seen.insert(rest.clone()) || seen.len() > 20_000 {
            return Err("duplicate or excessive archive entries".into());
        }
        let target = destination.join(&rest);
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
        } else if entry.header().entry_type().is_file() {
            let size = entry.size();
            if size > remaining {
                return Err("expanded source too large".into());
            }
            remaining -= size;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut file).map_err(|e| e.to_string())?;
            file.flush().map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = if entry.header().mode().map_err(|e| e.to_string())? & 0o111 != 0 {
                    0o755
                } else {
                    0o644
                };
                std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode))
                    .map_err(|e| e.to_string())?;
            }
        } else if entry.header().entry_type().is_symlink() {
            let link = entry
                .link_name()
                .map_err(|e| e.to_string())?
                .ok_or("missing source link target")?
                .into_owned();
            cyber_core::eval::validate_source_link(&rest, &link).map_err(|e| e.to_string())?;
            links.push((rest, link));
        } else {
            return Err("source hardlinks and special files are unsupported".into());
        }
    }
    // Defer aliases so no archive entry can be written through one.
    for (path, link) in links {
        let target = destination.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut parent = destination.to_path_buf();
        for part in path.parent().into_iter().flat_map(Path::components) {
            parent.push(part);
            if std::fs::symlink_metadata(&parent)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
            {
                return Err("symlink archive parent".into());
            }
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&link, &target).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            if destination
                .join(path.parent().unwrap_or(Path::new("")))
                .join(&link)
                .is_dir()
            {
                std::os::windows::fs::symlink_dir(&link, &target).map_err(|e| e.to_string())?;
            } else {
                std::os::windows::fs::symlink_file(&link, &target).map_err(|e| e.to_string())?;
            }
        }
        #[cfg(not(any(unix, windows)))]
        return Err("source links unsupported on this platform".into());
    }
    if seen.is_empty() {
        return Err("empty source archive".into());
    }
    Ok(revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(kind: tar::EntryType) -> Vec<u8> {
        let mut output = Vec::new();
        {
            let gzip = flate2::write::GzEncoder::new(&mut output, flate2::Compression::default());
            let mut archive = tar::Builder::new(gzip);
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(1);
            header.set_entry_type(kind);
            if kind.is_symlink() {
                header.set_link_name("../../outside").unwrap();
            }
            header.set_cksum();
            archive
                .append_data(&mut header, "source/file", &b"x"[..])
                .unwrap();
            archive.into_inner().unwrap().finish().unwrap();
        }
        output
    }

    #[test]
    fn pinned_import_preserves_regular_source_and_refuses_wrong_hash_or_links() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = archive(tar::EntryType::Regular);
        let hash = format!("{:x}", Sha256::digest(&bytes));
        unpack(&bytes, &dir.path().join("ok"), &hash).unwrap();
        assert_eq!(std::fs::read(dir.path().join("ok/file")).unwrap(), b"x");
        assert!(unpack(&bytes, &dir.path().join("bad"), &"0".repeat(64)).is_err());
        let bytes = archive(tar::EntryType::Symlink);
        assert!(
            unpack(
                &bytes,
                &dir.path().join("link"),
                &format!("{:x}", Sha256::digest(&bytes))
            )
            .is_err()
        );
        assert!(!dir.path().join("outside").exists());
    }

    fn raw_entries(paths: &[(&str, tar::EntryType, Option<&str>)]) -> Vec<u8> {
        let mut output = Vec::new();
        {
            let gzip = flate2::write::GzEncoder::new(&mut output, flate2::Compression::default());
            let mut archive = tar::Builder::new(gzip);
            for (path, kind, link) in paths {
                let mut header = tar::Header::new_gnu();
                header.set_mode(0o644);
                header.set_size(0);
                header.set_entry_type(*kind);
                let raw = header.as_mut_bytes();
                raw[..100].fill(0);
                raw[..path.len()].copy_from_slice(path.as_bytes());
                if let Some(link) = link {
                    header.set_link_name(link).unwrap();
                }
                header.set_cksum();
                archive.append(&header, std::io::empty()).unwrap();
            }
            archive.into_inner().unwrap().finish().unwrap();
        }
        output
    }

    #[test]
    fn archive_paths_cannot_escape_or_replace_metadata_or_write_through_aliases() {
        let cases = vec![
            vec![("source/../outside", tar::EntryType::Regular, None)],
            vec![("source/.Git/config", tar::EntryType::Regular, None)],
            vec![
                ("source/file", tar::EntryType::Regular, None),
                ("source/file", tar::EntryType::Regular, None),
            ],
            vec![
                ("source/alias", tar::EntryType::Symlink, Some("elsewhere")),
                ("source/alias/file", tar::EntryType::Regular, None),
            ],
            vec![("source/file", tar::EntryType::Link, Some("../../outside"))],
        ];
        for entries in cases {
            let root = tempfile::tempdir().unwrap();
            let bytes = raw_entries(&entries);
            assert!(
                unpack(
                    &bytes,
                    &root.path().join("source"),
                    &format!("{:x}", Sha256::digest(&bytes))
                )
                .is_err(),
                "{entries:?}"
            );
            assert!(!root.path().join("outside").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn source_alias_is_retained_without_copying_its_target_bytes() {
        let entries = [
            ("source/pkg/brew/file", tar::EntryType::Regular, None),
            (
                "source/HomebrewFormula",
                tar::EntryType::Symlink,
                Some("pkg/brew"),
            ),
        ];
        let root = tempfile::tempdir().unwrap();
        let bytes = raw_entries(&entries);
        unpack(
            &bytes,
            root.path(),
            &format!("{:x}", Sha256::digest(&bytes)),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_link(root.path().join("HomebrewFormula")).unwrap(),
            Path::new("pkg/brew")
        );
        assert!(cyber_core::eval::tree_sha256(root.path()).is_ok());
    }

    #[test]
    fn github_global_revision_metadata_is_not_a_second_source_root() {
        let mut bytes = Vec::new();
        {
            let gzip = flate2::write::GzEncoder::new(&mut bytes, flate2::Compression::default());
            let mut archive = tar::Builder::new(gzip);
            let data = format!("52 comment={}\n", "a".repeat(40));
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(data.len() as u64);
            header.set_entry_type(tar::EntryType::XGlobalHeader);
            header.set_cksum();
            archive
                .append_data(&mut header, "pax_global_header", data.as_bytes())
                .unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(1);
            header.set_cksum();
            archive
                .append_data(&mut header, "source/file", &b"x"[..])
                .unwrap();
            archive.into_inner().unwrap().finish().unwrap();
        }
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            unpack(
                &bytes,
                root.path(),
                &format!("{:x}", Sha256::digest(&bytes))
            )
            .unwrap(),
            Some("a".repeat(40))
        );
        assert_eq!(std::fs::read(root.path().join("file")).unwrap(), b"x");
    }
}
