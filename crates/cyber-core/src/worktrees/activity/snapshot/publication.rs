//! Complete locked records precede stable publication; incomplete stages remain evidence.
use super::super::{CheckoutLease, LockedRecord, Managed, UseRecord, invalid};
use super::{CheckoutBinding, CheckoutOwner, Pins};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::{fs::File, io};

pub(in crate::worktrees::activity) fn publish(
    managed: &Managed,
    activity: &str,
    lifetime: Option<CheckoutOwner>,
) -> io::Result<CheckoutLease> {
    publish_before(managed, activity, lifetime, |_| Ok(()))
}
fn publish_before(
    managed: &Managed,
    activity: &str,
    lifetime: Option<CheckoutOwner>,
    before_publish: impl FnOnce(&CheckoutBinding) -> io::Result<()>,
) -> io::Result<CheckoutLease> {
    super::super::safe_id(activity, "op")?;
    if let Some(owner) = &lifetime {
        owner.validate(managed)?;
        if owner.activity_id != activity {
            return Err(invalid("Foreign snapshot checkout activity"));
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Lifetime-bound checkout publication requires native directory durability",
        ));
    }
    super::super::use_directory(managed, true)?;
    let pins = Pins::open(&managed.common_dir, &managed.id, &managed.name)?;
    let directory = &pins.directory;
    let stable = format!("{activity}.lock");
    let prefix = format!("preparing_{}_{activity}_", activity.len());
    for (index, entry) in directory.entries()?.enumerate() {
        if index >= 4095 {
            return Err(invalid(
                "Checkout activity inventory exceeds limit; acknowledged cleanup is required",
            ));
        }
        if entry?.file_name().to_string_lossy().starts_with(&prefix) {
            return Err(invalid(
                "Snapshot checkout construction evidence is retained; recovery is required",
            ));
        }
    }
    match directory.symlink_metadata(&stable) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Snapshot checkout claim already exists",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let stage = format!("{prefix}{}", ulid::Ulid::new());
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = directory.open_with(&stage, &options)?.into_std();
    if !super::super::try_lock(&file)? {
        return Err(invalid("Snapshot checkout construction lock is busy"));
    }
    let mut file = LockedRecord(file);
    let mut binding = pins.capture(managed, activity, &file)?;
    binding.version = if lifetime.is_some() { 2 } else { 1 };
    binding.lifetime = lifetime;
    binding.staging_name = Some(stage.clone());
    let record = UseRecord {
        worktree_id: managed.id.clone(),
        owner_id: activity.into(),
        settled: false,
        snapshot_binding: Some(binding.clone()),
    };
    super::super::write_record(&mut file, &record)?;
    sync_file(&file)?;
    sync_directory(directory)?;
    pins.verify_at(&file, &record, &stage)?;
    before_publish(&binding)?;
    pins.verify_at(&file, &record, &stage)?;
    publish_name(directory, &stage, &stable)?;
    sync_directory(directory)?;
    let lease = CheckoutLease {
        file,
        record,
        snapshot_pins: Some(pins),
    };
    lease.verify_snapshot()?;
    Ok(lease)
}

fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()?;
    #[cfg(target_os = "macos")]
    rustix::fs::fcntl_fullfsync(file)?;
    Ok(())
}
fn sync_directory(directory: &Dir) -> io::Result<()> {
    #[cfg(unix)]
    {
        let file = rustix::fs::openat(
            directory,
            ".",
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )?;
        File::from(file).sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}
fn publish_name(directory: &Dir, stage: &str, stable: &str) -> io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        rustix::fs::renameat_with(
            directory,
            stage,
            directory,
            stable,
            rustix::fs::RenameFlags::NOREPLACE,
        )?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        // No replacement. An interrupted transition retains both names and must
        // fail alias verification; this is not lifetime-bound durable admission.
        directory.hard_link(stage, directory, stable)?;
        directory.remove_file(stage)?;
    }
    Ok(())
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use std::io::Read;
    fn fixture(root: &std::path::Path) -> Managed {
        let common = root.join("common");
        let path = root.join("work");
        std::fs::create_dir_all(common.join("cyber-worktrees")).unwrap();
        std::fs::create_dir_all(&path).unwrap();
        let managed = Managed {
            id: "wt_fixture".into(),
            name: "fixture".into(),
            path,
            branch: "cyber/fixture".into(),
            base: "main".into(),
            common_dir: common,
            ready: true,
            included: vec![],
        };
        std::fs::write(
            managed.common_dir.join("cyber-worktrees/fixture.json"),
            serde_json::to_vec(&managed).unwrap(),
        )
        .unwrap();
        managed
    }
    fn owner(managed: &Managed) -> CheckoutOwner {
        CheckoutOwner {
            activity_id: "op_fixture".into(),
            session_id: "ses_fixture".into(),
            session_created_id: "evt_fixture".into(),
            directory: managed.path.clone(),
            lifetime_digest: "a".repeat(64),
        }
    }
    fn directory(managed: &Managed) -> std::path::PathBuf {
        managed
            .common_dir
            .join("cyber-worktree-activity")
            .join(&managed.id)
    }
    #[test]
    fn complete_durable_stage_precedes_stable_name_and_a_collision_never_replaces_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let managed = fixture(&temp.path().canonicalize().unwrap());
        let owner = owner(&managed);
        let result = publish_before(
            &managed,
            &owner.activity_id,
            Some(owner.clone()),
            |binding| {
                assert!(!directory(&managed).join("op_fixture.lock").exists());
                let bytes =
                    std::fs::read(directory(&managed).join(binding.staging_name.as_ref().unwrap()))
                        .unwrap();
                let record: UseRecord = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(record.snapshot_binding.as_ref(), Some(binding));
                assert_eq!(binding.lifetime.as_ref(), Some(&owner));
                std::fs::write(
                    directory(&managed).join("op_fixture.lock"),
                    b"foreign owner bytes",
                )
                .unwrap();
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(directory(&managed).join("op_fixture.lock")).unwrap(),
            b"foreign owner bytes"
        );
        assert_eq!(std::fs::read_dir(directory(&managed)).unwrap().count(), 2);
    }
    #[test]
    fn namespace_replacement_before_publication_retains_the_original_stage_without_installing_a_claim()
     {
        let temp = tempfile::tempdir().unwrap();
        let managed = fixture(&temp.path().canonicalize().unwrap());
        let owner = owner(&managed);
        let retained = directory(&managed).with_extension("retained");
        let result = publish_before(
            &managed,
            &owner.activity_id,
            Some(owner.clone()),
            |binding| {
                let path = directory(&managed);
                let stage = binding.staging_name.as_ref().unwrap();
                let bytes = std::fs::read(path.join(stage)).unwrap();
                std::fs::rename(&path, &retained).unwrap();
                std::fs::create_dir(&path).unwrap();
                std::fs::write(path.join(stage), &bytes).unwrap();
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!directory(&managed).join("op_fixture.lock").exists());
        assert!(!retained.join("op_fixture.lock").exists());
        let original = std::fs::read_dir(&retained)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let record: UseRecord = serde_json::from_slice(&std::fs::read(original).unwrap()).unwrap();
        assert_eq!(record.snapshot_binding.unwrap().lifetime, Some(owner));
    }

    #[test]
    #[ignore = "subprocess fixture invoked by the publication kill test"]
    fn snapshot_publication_child() {
        let root =
            std::path::PathBuf::from(std::env::var_os("CYBER_CHECKOUT_PUBLICATION_ROOT").unwrap());
        let managed = fixture(&root);
        let owner = owner(&managed);
        let _ = publish_before(
            &managed,
            &owner.activity_id,
            Some(owner.clone()),
            |binding| {
                std::fs::write(
                    root.join("ready.json"),
                    serde_json::to_vec(binding).unwrap(),
                )
                .unwrap();
                std::io::stdin().read_exact(&mut [0]).unwrap();
                Ok(())
            },
        );
    }
    #[test]
    fn killed_construction_keeps_locked_complete_stage_and_never_creates_or_replays_stable_claim() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "worktrees::activity::snapshot::publication::tests::snapshot_publication_child",
                "--ignored",
            ])
            .env("CYBER_CHECKOUT_PUBLICATION_ROOT", &root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let binding: CheckoutBinding = loop {
            if let Ok(bytes) = std::fs::read(root.join("ready.json"))
                && let Ok(binding) = serde_json::from_slice(&bytes)
            {
                break binding;
            }
            assert!(child.try_wait().unwrap().is_none());
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let managed: Managed = serde_json::from_slice(
            &std::fs::read(root.join("common/cyber-worktrees/fixture.json")).unwrap(),
        )
        .unwrap();
        let stage = directory(&managed).join(binding.staging_name.as_ref().unwrap());
        let file = File::open(&stage).unwrap();
        assert!(matches!(
            file.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(file.try_lock().is_ok());
        file.unlock().unwrap();
        let retained = std::fs::read(&stage).unwrap();
        assert!(!directory(&managed).join("op_fixture.lock").exists());
        assert!(publish(&managed, "op_fixture", binding.lifetime.clone()).is_err());
        assert_eq!(std::fs::read(stage).unwrap(), retained);
        let fresh = publish(&managed, "op_fresh", None).unwrap();
        fresh.verify_snapshot().unwrap();
    }
}
