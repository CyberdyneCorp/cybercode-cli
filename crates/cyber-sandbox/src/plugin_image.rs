//! Own preparation in a helper process, including cancellation and image lifetime.
//! The caller supplies a protected owner staging base and trusted helper binding.
//! This is not OS execution admission or host identity reconciliation.
use std::path::Path;
#[cfg(unix)]
use std::{path::PathBuf, time::Duration};

use cyber_core::plugins::image::PreparedCopy;
#[cfg(unix)]
use cyber_core::plugins::image::{MAX_BYTES, MAX_FILES, MAX_REQUEST, MAX_RESPONSE, Request};
#[cfg(unix)]
use tokio::io::{AsyncRead, AsyncReadExt};
#[cfg(unix)]
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("plugin package preparation is unavailable on this platform")]
    Unavailable,
    #[error("invalid plugin package preparation request")]
    Request,
    #[error("plugin package staging or helper IO failed")]
    Io,
    #[error("plugin package helper refused the requested copy")]
    Refused,
    #[error("plugin package helper exceeded its output limit")]
    Output,
    #[error("plugin package helper response did not match its request or directory binding")]
    Binding,
    #[error("plugin package preparation timed out")]
    Timeout,
    #[error("plugin package generation has been revoked")]
    Revoked,
}

mod authority;
pub mod private_roots;
pub use authority::{Generation, Lease};

/// An owned copy, not a sealed or execution-admitted image. Retain this value for
/// as long as a launcher or host uses the copy. Do not publish without checking
/// current generation authority, private-root provenance and native compatibility.
#[derive(Debug)]
pub struct Package {
    #[cfg(unix)]
    _root: cap_std::fs::Dir,
    #[cfg(unix)]
    _namespace: cap_std::fs::Dir,
    #[cfg(unix)]
    _workspace: cap_std::fs::File,
    _parent: tempfile::TempDir,
    report: PreparedCopy,
    lease: Lease,
}
impl Package {
    /// Re-check authority at use. A copied path is not a native launch permit;
    /// launch commit must still serialize generation revocation and process start.
    pub fn path(&self) -> Result<&Path, Error> {
        self.lease.check()?;
        Ok(&self.report.destination)
    }
    /// Serialize the final synchronous start with generation revocation. The
    /// callback must actually perform the start, not return a future to run later.
    /// This supplies only generation authority: private provenance, immutable
    /// runtime/helper binding and native OS admission are still caller obligations.
    /// Do not revoke the same generation from the callback (the gate is held).
    pub fn commit<T>(
        &self,
        owner: &Generation,
        start: impl FnOnce(&Path) -> T,
    ) -> Result<T, Error> {
        owner.commit(&self.lease, || start(&self.report.destination))
    }
    pub fn digest(&self) -> &str {
        &self.report.digest
    }
}

/// The staging base and all its ancestors must remain protected against untrusted
/// rename/write until the returned package drops. The helper must be the trusted
/// no-descendant package-copy worker. Dropping this future cancels preparation;
/// a supervisor retains the staging directory until the worker has been reaped.
/// Even on timeout, uninterruptible OS IO can delay reaping and therefore cleanup.
pub async fn prepare(
    lease: &Lease,
    helper: &Path,
    base: &Path,
    source: &Path,
    digest: &str,
) -> Result<Package, Error> {
    #[cfg(not(unix))]
    {
        let _ = (lease, helper, base, source, digest);
        Err(Error::Unavailable)
    }
    #[cfg(unix)]
    {
        let package = start(lease, helper, base, source, digest, Duration::from_secs(30)).await?;
        lease.check()?;
        Ok(package)
    }
}

#[cfg(unix)]
async fn start(
    lease: &Lease,
    helper: &Path,
    base: &Path,
    source: &Path,
    digest: &str,
    deadline: Duration,
) -> Result<Package, Error> {
    lease.check()?;
    if !helper.is_absolute() || digest.len() != 71 {
        return Err(Error::Request);
    }
    Request {
        source: source.into(),
        destination: base.join("image"),
        expected_digest: digest.into(),
    }
    .validate()
    .map_err(|_| Error::Request)?;
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .prefix("plugin-image-")
        .tempdir_in(base)
        .map_err(|_| Error::Io)?;
    let request = Request {
        source: source.into(),
        destination: parent.path().join("image"),
        expected_digest: digest.into(),
    };
    request.validate().map_err(|_| Error::Request)?;
    let frame = serde_json::to_string(&request).map_err(|_| Error::Request)?;
    if frame.len() > MAX_REQUEST {
        return Err(Error::Request);
    }
    let helper = helper.to_path_buf();
    let (reply, result) = oneshot::channel();
    // The worker and parent are owned by this task, not by the caller's future.
    // A dropped receiver is a cancellation request, never permission to detach IO.
    tokio::spawn(supervise(
        lease.clone(),
        helper,
        parent,
        request,
        frame,
        deadline,
        reply,
    ));
    result.await.map_err(|_| Error::Io)?
}

#[cfg(unix)]
async fn supervise(
    lease: Lease,
    helper: PathBuf,
    parent: tempfile::TempDir,
    request: Request,
    frame: String,
    deadline: Duration,
    mut reply: oneshot::Sender<Result<Package, Error>>,
) {
    use std::process::Stdio;
    if let Err(error) = lease.check() {
        let _ = reply.send(Err(error));
        return;
    }
    let child = tokio::process::Command::new(helper)
        .args(["--plugin-copy", &frame])
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .current_dir(parent.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let Ok(mut child) = child else {
        let _ = reply.send(Err(Error::Io));
        return;
    };
    let mut retained = RetainUntilReaped(Some(parent));
    let stdout = child.stdout.take().expect("piped helper stdout");
    let stderr = child.stderr.take().expect("piped helper stderr");
    let capture = async {
        let (stdout, _) = tokio::try_join!(bounded(stdout, MAX_RESPONSE), bounded(stderr, 8192))?;
        let status = child.wait().await.map_err(|_| Error::Io)?;
        if !status.success() {
            return Err(Error::Refused);
        }
        serde_json::from_slice::<PreparedCopy>(&stdout).map_err(|_| Error::Binding)
    };
    let result = tokio::select! {
        biased;
        _ = reply.closed() => None,
        _ = lease.revoked() => Some(Err(Error::Revoked)),
        _ = tokio::time::sleep(deadline) => Some(Err(Error::Timeout)),
        result = capture => Some(result),
    };
    // On every terminal path, settle the exact process before releasing its
    // writable parent. The trusted worker cannot spawn descendants.
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.start_kill();
    }
    if child.wait().await.is_err() {
        // Do not delete a namespace that might still have a live writer. Leaking
        // this protected parent is safer than removing it before a proven reap.
        let _ = reply.send(Err(Error::Io));
        return;
    }
    let parent = retained.0.take().expect("owned helper parent");
    if let Some(result) = result {
        let package = result.and_then(|report| bind(parent, &request, report, lease));
        let _ = reply.send(package);
    }
}

#[cfg(unix)]
async fn bounded(stream: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    stream
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| Error::Io)?;
    if bytes.len() > limit {
        return Err(Error::Output);
    }
    Ok(bytes)
}

#[cfg(unix)]
fn bind(
    parent: tempfile::TempDir,
    request: &Request,
    report: PreparedCopy,
    lease: Lease,
) -> Result<Package, Error> {
    lease.check()?;
    use cap_fs_ext::DirExt;
    use cap_std::fs::MetadataExt;
    if report.destination != request.destination
        || report.digest != request.expected_digest
        || report.files > MAX_FILES
        || report.bytes > MAX_BYTES
    {
        return Err(Error::Binding);
    }
    let directory = cap_std::fs::Dir::open_ambient_dir(parent.path(), cap_std::ambient_authority())
        .map_err(|_| Error::Binding)?;
    let root = directory
        .open_dir_nofollow("image")
        .map_err(|_| Error::Binding)?;
    let metadata = root.dir_metadata().map_err(|_| Error::Binding)?;
    if (metadata.dev(), metadata.ino()) != report.root_identity {
        return Err(Error::Binding);
    }
    // A Deno package config otherwise searches arbitrary ancestors for a
    // workspace, even when selected by an absolute --config argument. Keep this
    // static boundary outside the approved image, so its digest stays unchanged.
    // The caller's protected-namespace obligation applies to this file too.
    let workspace = bootstrap_boundary(&directory)?;
    Ok(Package {
        _root: root,
        _namespace: directory,
        _workspace: workspace,
        _parent: parent,
        report,
        lease,
    })
}

#[cfg(unix)]
fn bootstrap_boundary(directory: &cap_std::fs::Dir) -> Result<cap_std::fs::File, Error> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    const NAME: &str = "package.json";
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = directory.open_with(NAME, &options).map_err(|_| Error::Io)?;
    file.write_all(b"{\"private\":true,\"workspaces\":[]}\n")
        .map_err(|_| Error::Io)?;
    file.sync_all().map_err(|_| Error::Io)?;
    file.set_permissions(cap_std::fs::Permissions::from_std(
        std::fs::Permissions::from_mode(0o400),
    ))
    .map_err(|_| Error::Io)?;
    drop(file);
    directory.open(NAME).map_err(|_| Error::Io)
}

/// Runtime shutdown can drop the supervisor before its asynchronous reap ends.
/// In that case retain the protected namespace rather than racing a live writer.
#[cfg(unix)]
struct RetainUntilReaped(Option<tempfile::TempDir>);
#[cfg(unix)]
impl Drop for RetainUntilReaped {
    fn drop(&mut self) {
        if let Some(parent) = self.0.take() {
            let _retained = parent.keep();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn workspace_boundary_refuses_existing_files_and_symlinks_without_modifying_them() {
        let namespace = tempfile::tempdir().unwrap();
        let directory =
            cap_std::fs::Dir::open_ambient_dir(namespace.path(), cap_std::ambient_authority())
                .unwrap();
        let boundary = namespace.path().join("package.json");
        std::fs::write(&boundary, b"existing metadata").unwrap();
        assert!(matches!(bootstrap_boundary(&directory), Err(Error::Io)));
        assert_eq!(std::fs::read(&boundary).unwrap(), b"existing metadata");
        std::fs::remove_file(&boundary).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), b"outside metadata").unwrap();
        std::os::unix::fs::symlink(outside.path(), &boundary).unwrap();
        assert!(matches!(bootstrap_boundary(&directory), Err(Error::Io)));
        assert_eq!(std::fs::read(outside.path()).unwrap(), b"outside metadata");
        assert!(std::fs::symlink_metadata(&boundary).unwrap().is_symlink());
    }

    fn worker(root: &Path, control: u8) -> PathBuf {
        let helper = root.join(format!("helper-{control}"));
        let output = std::process::Command::new("cc")
            .arg(format!("-DCONTROL={control}"))
            .args(["-Wall", "-Wextra", "-Werror", "-o"])
            .arg(&helper)
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugin_image_owner.c"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        helper
    }
    async fn receipt(base: &Path) -> u32 {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                for entry in std::fs::read_dir(base).unwrap() {
                    if let Ok(contents) =
                        std::fs::read_to_string(entry.unwrap().path().join("alive"))
                        && let Ok(pid) = contents.parse()
                    {
                        return pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker must actually start")
    }
    #[tokio::test]
    async fn invalid_requests_are_refused_before_creating_staging_or_spawning() {
        let generation = Generation::new();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let base = root.join("not-created");
        let helper = root.join("missing-helper");
        for (source, digest) in [
            (root.join("source"), "x".repeat(1024 * 1024)),
            (root.join("source"), format!("sha256:{}", "A".repeat(64))),
            (
                root.join("../outside"),
                format!("sha256:{}", "0".repeat(64)),
            ),
        ] {
            assert_eq!(
                prepare(&generation.lease(), &helper, &base, &source, &digest)
                    .await
                    .unwrap_err(),
                Error::Request
            );
            assert!(!base.exists());
        }
    }
    #[tokio::test]
    async fn malformed_refused_and_flooded_workers_leave_no_output_or_payload_in_errors() {
        let generation = Generation::new();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let base = root.join("staging");
        std::fs::create_dir(&base).unwrap();
        for (control, expected) in [
            (0, Error::Binding),
            (2, Error::Output),
            (3, Error::Output),
            (4, Error::Refused),
        ] {
            let error = prepare(
                &generation.lease(),
                &worker(&root, control),
                &base,
                &root.join("source"),
                &format!("sha256:{}", "0".repeat(64)),
            )
            .await
            .unwrap_err();
            assert_eq!(error, expected);
            assert!(!error.to_string().contains("PRIVATE"));
            assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);
        }
    }
    #[tokio::test]
    async fn dropping_the_request_kills_and_reaps_the_live_worker_before_cleanup() {
        let generation = Generation::new();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let base = root.join("staging");
        std::fs::create_dir(&base).unwrap();
        let helper = worker(&root, 1);
        let task_base = base.clone();
        let lease = generation.lease();
        let task = tokio::spawn(async move {
            prepare(
                &lease,
                &helper,
                &task_base,
                &task_base.join("source"),
                &format!("sha256:{}", "0".repeat(64)),
            )
            .await
        });
        let pid = receipt(&base).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(5), async {
            while std::fs::read_dir(&base).unwrap().count() != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled worker must be reaped and cleaned");
        let status = std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .output()
            .unwrap();
        assert!(
            !status.status.success(),
            "worker still exists after cleanup"
        );
    }
    #[tokio::test]
    async fn timeout_reaps_the_worker_before_returning_and_removes_partial_namespace() {
        let generation = Generation::new();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let base = root.join("staging");
        std::fs::create_dir(&base).unwrap();
        let error = start(
            &generation.lease(),
            &worker(&root, 1),
            &base,
            &root.join("source"),
            &format!("sha256:{}", "0".repeat(64)),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert_eq!(error, Error::Timeout);
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);
    }
    #[test]
    fn dropping_an_unsettled_supervisor_retains_the_namespace() {
        let temp = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir_in(temp.path()).unwrap();
        let path = parent.path().to_owned();
        drop(RetainUntilReaped(Some(parent)));
        assert!(
            path.is_dir(),
            "a dropped supervisor cannot delete a live worker namespace"
        );
        // This guard-only control has no live worker, so cleanup is safe here.
        std::fs::remove_dir(&path).unwrap();
    }
    #[test]
    fn a_forged_root_identity_is_refused_even_with_matching_path_and_digest() {
        let generation = Generation::new();
        let temp = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir_in(temp.path()).unwrap();
        let destination = parent.path().join("image");
        std::fs::create_dir(&destination).unwrap();
        let metadata = std::fs::metadata(&destination).unwrap();
        let request = Request {
            source: temp.path().join("source"),
            destination: destination.clone(),
            expected_digest: "digest".into(),
        };
        let report = PreparedCopy {
            destination,
            root_identity: (metadata.dev(), metadata.ino().wrapping_add(1)),
            digest: "digest".into(),
            files: 0,
            bytes: 0,
        };
        assert_eq!(
            bind(parent, &request, report, generation.lease()).unwrap_err(),
            Error::Binding
        );
    }
    #[tokio::test]
    async fn revoking_the_generation_reaps_active_preparation_before_returning() {
        let generation = Generation::new();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let base = root.join("staging");
        std::fs::create_dir(&base).unwrap();
        let helper = worker(&root, 1);
        let lease = generation.lease();
        let task_base = base.clone();
        let mut task = tokio::spawn(async move {
            prepare(
                &lease,
                &helper,
                &task_base,
                &task_base.join("source"),
                &format!("sha256:{}", "0".repeat(64)),
            )
            .await
        });
        let pid = receipt(&base).await;
        generation.revoke();
        let outcome = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
        if outcome.is_err() {
            task.abort();
            let _ = task.await;
            tokio::time::timeout(Duration::from_secs(5), async {
                while std::fs::read_dir(&base).unwrap().count() != 0 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("failed control must settle the worker too");
        }
        let error = outcome
            .expect("revocation must settle without waiting for the deadline")
            .unwrap()
            .unwrap_err();
        assert_eq!(error, Error::Revoked);
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);
        let exists = std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .output()
            .unwrap();
        assert!(
            !exists.status.success(),
            "revoked worker must be reaped before return"
        );
    }
}

#[cfg(test)]
mod authority_tests {
    use super::*;
    #[test]
    fn dropping_authority_revokes_cloned_leases_without_renewal() {
        let generation = Generation::new();
        let first = generation.lease();
        let clone = first.clone();
        assert!(first.is_current());
        drop(generation);
        assert!(!first.is_current());
        assert!(!clone.is_current());
        assert_eq!(clone.check(), Err(Error::Revoked));
    }
    #[test]
    fn a_new_generation_never_readmits_previous_leases() {
        let previous = Generation::new();
        let old = previous.lease();
        previous.revoke();
        previous.revoke();
        assert!(!previous.lease().is_current());
        let current = Generation::new();
        assert!(current.lease().is_current());
        assert!(!old.is_current());
    }
}
