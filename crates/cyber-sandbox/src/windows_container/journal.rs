//! Durable records of AppContainer grants (`sandbox` → Windows enforcement).
//!
//! A confined launch's helper runs inside the `--job` process-tree owner, so a timeout,
//! cancel or host shutdown terminates it together with the command and none of its
//! cleanup runs. Before it grants anything the helper therefore writes
//! `<journal>/<profile>.json` (the profile name and every granted path) and holds it open
//! without sharing for as long as it lives. A journal that can be opened belongs to a
//! helper that is gone: [`sweep`] removes that identity's entries from every object under
//! the recorded paths, deletes its profile and then the journal.

use std::io::{Read as _, Write as _};

use windows_sys::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;

use super::*;

/// The prefix of every profile [`Profile::new`] creates; a journal naming anything else
/// is never acted on.
const PROFILE_PREFIX: &str = "cyberdyne.cyber.box_";

/// A live helper's record of its grants.
pub struct GrantJournal {
    file: Option<File>,
    path: PathBuf,
}

impl GrantJournal {
    /// Record `profile` and the paths it is about to be granted, before any grant.
    pub fn create(dir: &Path, profile: &Profile, paths: &[PathBuf]) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.json", profile.name()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(&path)?;
        let paths: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        let text = serde_json::json!({"profile": profile.name(), "paths": paths}).to_string();
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(Self {
            file: Some(file),
            path,
        })
    }

    /// Every grant is revoked and the profile deleted: the record goes.
    pub fn finish(mut self) -> io::Result<()> {
        drop(self.file.take());
        std::fs::remove_file(&self.path)
    }
}

/// Revoke what every journal in `dir` whose helper is gone still grants, visiting at
/// most `limit` objects per recorded path. Failures leave that journal for a later sweep.
pub fn sweep(dir: &Path, limit: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json")
            && let Err(error) = recover(&path, limit)
            && error.raw_os_error() != Some(SHARING_VIOLATION)
        {
            cleanup_error("stale AppContainer grant recovery", error);
        }
    }
}

/// `ERROR_SHARING_VIOLATION`: the journal's helper still holds it.
const SHARING_VIOLATION: i32 = 32;

fn recover(path: &Path, limit: usize) -> io::Result<()> {
    let mut file = OpenOptions::new().read(true).share_mode(0).open(path)?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let record: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let profile = record["profile"]
        .as_str()
        .filter(|name| name.starts_with(PROFILE_PREFIX) && name.is_ascii());
    // A helper killed while writing its journal had granted nothing yet.
    if let Some(profile) = profile {
        let sid = derived_sid(profile)?;
        let paths = record["paths"].as_array().into_iter().flatten();
        for root in paths.filter_map(serde_json::Value::as_str) {
            revoke_tree(Path::new(root), sid.0, limit);
        }
        delete_profile(profile);
    }
    drop(file);
    std::fs::remove_file(path)
}

/// The SID Windows derives for the AppContainer `name`.
fn derived_sid(name: &str) -> io::Result<AllocatedSid> {
    let encoded = wide(name);
    let mut raw = null_mut();
    // The name stays valid through the call; the SID is freed by `AllocatedSid`.
    let status = unsafe { DeriveAppContainerSidFromAppContainerName(encoded.as_ptr(), &mut raw) };
    let sid = AllocatedSid(raw);
    hresult(status)?;
    Ok(sid)
}

fn delete_profile(name: &str) {
    let encoded = wide(name);
    // An already deleted profile is the outcome sought; nothing to report.
    let _ = unsafe { DeleteAppContainerProfile(encoded.as_ptr()) };
}

/// Remove every entry of `sid` (granted, denied or inherited) from `root` and the
/// objects below it, never following links or junctions.
fn revoke_tree(root: &Path, sid: PSID, limit: usize) {
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0;
    while let Some(path) = pending.pop() {
        if visited >= limit {
            return;
        }
        visited += 1;
        let Ok(file) = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
        else {
            continue;
        };
        let _ = remove_profile_entries(&file, sid);
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            let children = std::fs::read_dir(&path).into_iter().flatten().flatten();
            pending.extend(children.map(|entry| entry.path()));
        }
    }
}

/// Whether `path` still carries an entry for the AppContainer `name`.
pub fn has_entries(path: &Path, name: &str) -> io::Result<bool> {
    let sid = derived_sid(name)?;
    let file = OpenOptions::new()
        .access_mode(READ_CONTROL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let (_descriptor, acl) = identity::read_acl(&file)?;
    if acl.is_null() {
        return Ok(false);
    }
    Ok(!sid_entries(acl, sid.0)?.is_empty())
}

/// Whether the AppContainer profile `name` is still registered.
pub fn profile_registered(name: &str) -> io::Result<bool> {
    let encoded = wide(name);
    let mut raw = null_mut();
    // Creating the profile is the only query: success means it was absent, so the
    // probe's own profile is deleted again at once.
    let status = unsafe {
        CreateAppContainerProfile(
            encoded.as_ptr(),
            encoded.as_ptr(),
            encoded.as_ptr(),
            null(),
            0,
            &mut raw,
        )
    };
    let _sid = AllocatedSid(raw);
    const ALREADY_EXISTS: i32 = 0x800700B7_u32 as i32;
    match status {
        ALREADY_EXISTS => Ok(true),
        status if status >= 0 => {
            delete_profile(name);
            Ok(false)
        }
        status => hresult(status).map(|()| false),
    }
}
