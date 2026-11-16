//! Owner-confirmed discard of one checkout claim or acknowledgement construction
//! stage. Review is read-only. The fence holds the stage's own constructor lock
//! exclusively (a live constructor keeps it from creation through publication) and
//! checks the activity's published records before the caller may quarantine it.
use super::super::super::{Managed, Repository};
use super::super::{LockedRecord, invalid};
use super::constructions::{SnapshotConstruction, SnapshotConstructionKind, classify};
use crate::construction_discard::{self as native, StageEvidence};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use std::io;
use std::path::{Path, PathBuf};

const ROOT: &str = "cyber-worktree-activity";

/// A fenced stage: its constructor lock is held until this value is dropped.
pub struct CheckoutConstructionFence {
    root: Dir,
    activity: Dir,
    file: LockedRecord,
    name: String,
    evidence: StageEvidence,
}

impl CheckoutConstructionFence {
    /// Move the fenced stage into `discarded/<key>/` beside the activity directories.
    pub fn quarantine(&self, key: &str) -> io::Result<()> {
        native::quarantine(
            &self.activity,
            &self.name,
            &self.root,
            key,
            &self.file,
            &self.evidence,
        )
    }
}

fn managed_at(directory: &Path, worktree_id: &str) -> io::Result<Option<Managed>> {
    Ok(Repository::managed_locations_at(directory)?
        .into_iter()
        .map(|(_, managed)| managed)
        .find(|managed| managed.id == worktree_id))
}

/// The private activity namespace and this checkout's activity directory.
fn namespace(managed: &Managed) -> io::Result<(Dir, Dir, PathBuf)> {
    super::super::safe_id(&managed.id, "wt")?;
    if std::fs::symlink_metadata(&managed.common_dir)?
        .file_type()
        .is_symlink()
    {
        return Err(invalid("Checkout common directory is a symlink"));
    }
    let common = Dir::open_ambient_dir(&managed.common_dir, cap_std::ambient_authority())?;
    let root = common.open_dir_nofollow(ROOT)?;
    let activity = root.open_dir_nofollow(&managed.id)?;
    Ok((root, activity, managed.common_dir.join(ROOT)))
}

/// A reviewed checkout stage and where it lives.
pub struct CheckoutConstructionReview {
    pub stage: SnapshotConstruction,
    pub evidence: StageEvidence,
    /// The activity directory that holds the stage.
    pub source: PathBuf,
    /// The namespace whose `discarded/` receives it.
    pub namespace: PathBuf,
}

impl Repository {
    /// Read-only review of one claim or acknowledgement stage of the managed
    /// checkout `worktree_id` enclosing `directory`. `None` when there is no such
    /// stage; nothing is opened for writing, locked or adopted.
    pub fn review_snapshot_construction(
        directory: &Path,
        worktree_id: &str,
        name: &str,
    ) -> io::Result<Option<CheckoutConstructionReview>> {
        let Some((kind, owner_id)) = classify(name) else {
            return Ok(None);
        };
        let Some(managed) = managed_at(directory, worktree_id)? else {
            return Ok(None);
        };
        let (_, activity, namespace) = match namespace(&managed) {
            Ok(found) => found,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let evidence = match native::observe(&activity, name) {
            Ok((_, evidence)) => evidence,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(Some(CheckoutConstructionReview {
            stage: SnapshotConstruction {
                worktree_id: managed.id.clone(),
                owner_id,
                kind,
                name: name.into(),
            },
            evidence,
            source: namespace.join(&managed.id),
            namespace,
        }))
    }

    /// Fence one reviewed stage for discard: the name must parse to `owner`, its
    /// constructor lock must be free, the stage must still have the reviewed
    /// evidence, and the activity's published records must allow it (no published
    /// claim for a claim stage; an acknowledgement published from another stage
    /// for an acknowledgement stage).
    pub fn fence_snapshot_construction(
        directory: &Path,
        worktree_id: &str,
        name: &str,
        owner: &str,
        expected: &StageEvidence,
    ) -> io::Result<CheckoutConstructionFence> {
        let (kind, parsed) =
            classify(name).ok_or_else(|| invalid("Not a checkout construction stage"))?;
        if parsed.as_deref() != Some(owner) {
            return Err(invalid(
                "Construction stage name has no derivable owner; it stays fenced",
            ));
        }
        let managed = managed_at(directory, worktree_id)?
            .ok_or_else(|| invalid("Managed checkout of this Session is unavailable"))?;
        fence_managed(&managed, kind, name, owner, expected)
    }
}

fn fence_managed(
    managed: &Managed,
    kind: SnapshotConstructionKind,
    name: &str,
    owner: &str,
    expected: &StageEvidence,
) -> io::Result<CheckoutConstructionFence> {
    let (root, activity, _) = namespace(managed)?;
    let (file, observed) = native::observe(&activity, name)?;
    if !native::try_lock(&file)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Checkout constructor still holds this stage",
        ));
    }
    let file = LockedRecord(file);
    if observed != *expected || native::evidence(&file)? != *expected {
        return Err(invalid("Construction stage changed since review"));
    }
    published_records_allow(&activity, kind, owner, name)?;
    Ok(CheckoutConstructionFence {
        root,
        activity,
        file,
        name: name.into(),
        evidence: expected.clone(),
    })
}

fn published_records_allow(
    activity: &Dir,
    kind: SnapshotConstructionKind,
    owner: &str,
    name: &str,
) -> io::Result<()> {
    match kind {
        SnapshotConstructionKind::CheckoutClaim => {
            match activity.symlink_metadata(format!("{owner}.lock")) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
                Ok(_) => Err(invalid(format!(
                    "A checkout claim is published for {owner}; ordinary recovery applies"
                ))),
            }
        }
        SnapshotConstructionKind::CheckoutAcknowledgement => {
            match super::acknowledgement::published_stage(activity, owner)? {
                None => Err(invalid(format!(
                    "The claim of {owner} is not acknowledged yet; reconcile its receipt \
                     (recovery-preparation-request --acknowledge) before discarding this stage"
                ))),
                Some(stage) if stage == name => Err(invalid(
                    "The published acknowledgement names this stage; ordinary recovery applies",
                )),
                Some(_) => Ok(()),
            }
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    fn fixture(root: &Path) -> (Managed, PathBuf) {
        let common = root.join("common");
        let activity = common.join(ROOT).join("wt_fixture");
        std::fs::create_dir_all(&activity).unwrap();
        let managed = Managed {
            id: "wt_fixture".into(),
            name: "fixture".into(),
            path: root.join("work"),
            branch: "cyber/fixture".into(),
            base: "main".into(),
            common_dir: common,
            ready: true,
            included: vec![],
        };
        (managed, activity)
    }
    const KEY: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    fn claim() -> String {
        format!("preparing_10_op_fixture_{}", ulid::Ulid::new())
    }

    fn fence(managed: &Managed, name: &str, owner: &str) -> io::Result<CheckoutConstructionFence> {
        let (_, activity, _) = namespace(managed)?;
        let (_, evidence) = native::observe(&activity, name)?;
        let (kind, _) = classify(name).unwrap();
        fence_managed(managed, kind, name, owner, &evidence)
    }

    #[test]
    fn unpublished_claim_stage_is_quarantined_and_unlinked() {
        let temp = tempfile::tempdir().unwrap();
        let (managed, activity) = fixture(temp.path());
        let name = claim();
        std::fs::write(activity.join(&name), b"{}").unwrap();
        let fenced = fence(&managed, &name, "op_fixture").unwrap();
        fenced.quarantine(KEY).unwrap();
        drop(fenced);
        assert!(!activity.join(&name).exists());
        let (root, _, _) = namespace(&managed).unwrap();
        let evidence_path = managed
            .common_dir
            .join(ROOT)
            .join("discarded")
            .join(KEY)
            .join(&name);
        assert!(evidence_path.exists());
        let (_, evidence) = native::observe(
            &root.open_dir("discarded").unwrap().open_dir(KEY).unwrap(),
            &name,
        )
        .unwrap();
        assert_eq!(
            native::unlink(&root, KEY, &name, &evidence).unwrap(),
            native::Place::Gone
        );
        assert!(!evidence_path.exists());
    }

    #[test]
    fn a_held_stage_lock_or_a_published_claim_refuses() {
        let temp = tempfile::tempdir().unwrap();
        let (managed, activity) = fixture(temp.path());
        let name = claim();
        std::fs::write(activity.join(&name), b"{}").unwrap();
        let held = std::fs::File::open(activity.join(&name)).unwrap();
        held.lock().unwrap();
        let error = fence(&managed, &name, "op_fixture").err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        held.unlock().unwrap();
        std::fs::write(activity.join("op_fixture.lock"), b"{}").unwrap();
        let error = fence(&managed, &name, "op_fixture").err().unwrap();
        assert!(error.to_string().contains("ordinary recovery"), "{error}");
        assert!(activity.join(&name).exists());
    }

    #[test]
    fn acknowledgement_stage_waits_for_reconciliation() {
        let temp = tempfile::tempdir().unwrap();
        let (managed, activity) = fixture(temp.path());
        let name = format!("ack_preparing_10_op_fixture_{}", ulid::Ulid::new());
        std::fs::write(activity.join(&name), b"{}").unwrap();
        let error = fence(&managed, &name, "op_fixture").err().unwrap();
        assert!(
            error.to_string().contains("not acknowledged yet"),
            "{error}"
        );
        assert!(activity.join(&name).exists());
    }

    #[test]
    fn malformed_or_foreign_stage_names_have_no_owner() {
        assert!(classify("preparing_x_op_fixture").is_some_and(|(_, owner)| owner.is_none()));
        let temp = tempfile::tempdir().unwrap();
        let error = Repository::fence_snapshot_construction(
            temp.path(),
            "wt_fixture",
            "preparing_bad",
            "op_fixture",
            &StageEvidence {
                device: 0,
                inode: 0,
                size: 0,
                modified_ns: 0,
                sha256: String::new(),
            },
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("no derivable owner"), "{error}");
    }
}
