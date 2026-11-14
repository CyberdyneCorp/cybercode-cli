//! Publish an answer without sharing temporary paths with other writers.
use std::io::{self, Write};
use std::path::Path;

pub(super) fn write(path: &Path, text: &str) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(".cyber-last-message-")
        .tempfile_in(parent)?;
    temporary.write_all(text.as_bytes())?;
    temporary.as_file().sync_all()?;
    publish(temporary, path)
}

/// Windows refuses a rename onto a destination another writer is replacing at the same
/// moment ("Access is denied"); the competing rename finishes within milliseconds.
const BUSY_RETRIES: u32 = 50;
const BUSY_PAUSE: std::time::Duration = std::time::Duration::from_millis(10);

fn publish(mut temporary: tempfile::NamedTempFile, path: &Path) -> io::Result<()> {
    let mut retries = 0;
    loop {
        match temporary.persist(path) {
            Ok(_) => return Ok(()),
            Err(error) if retries < BUSY_RETRIES && destination_busy(&error.error) => {
                temporary = error.file;
                retries += 1;
                std::thread::sleep(BUSY_PAUSE);
            }
            Err(error) => return Err(error.error),
        }
    }
}

fn destination_busy(error: &io::Error) -> bool {
    cfg!(windows) && error.kind() == io::ErrorKind::PermissionDenied
}

#[cfg(test)]
mod tests {
    use super::write;

    #[cfg(unix)]
    #[test]
    fn existing_temporary_and_destination_links_do_not_redirect_writes() {
        let dir = tempfile::tempdir().unwrap();
        let protected = dir.path().join("protected");
        std::fs::write(&protected, "owner bytes").unwrap();
        let path = dir.path().join("answer");
        let predictable = dir.path().join(".answer.tmp");
        std::os::unix::fs::symlink(&protected, &predictable).unwrap();
        std::os::unix::fs::symlink(&protected, &path).unwrap();
        write(&path, "answer bytes").unwrap();
        assert_eq!(std::fs::read_to_string(protected).unwrap(), "owner bytes");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "answer bytes");
        assert!(std::fs::symlink_metadata(predictable).unwrap().is_symlink());
        assert!(std::fs::symlink_metadata(path).unwrap().is_file());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn concurrent_writers_publish_one_complete_answer_and_remove_their_stages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("answer");
        let answers: Vec<_> = (0..8)
            .map(|i| format!("answer {i}\n").repeat(8192))
            .collect();
        let barrier = std::sync::Barrier::new(answers.len());
        std::thread::scope(|scope| {
            for answer in &answers {
                let (path, barrier) = (&path, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    write(path, answer).unwrap();
                });
            }
        });
        assert!(answers.contains(&std::fs::read_to_string(path).unwrap()));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_publication_preserves_the_destination_and_removes_its_stage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("answer");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("owner"), "preserved").unwrap();
        assert!(write(&path, "answer bytes").is_err());
        assert_eq!(
            std::fs::read_to_string(path.join("owner")).unwrap(),
            "preserved"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
