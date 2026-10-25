use base64::Engine as _;
use cyber_computer::{backend::Capture, control::Frame};
use cyber_llm::{Content, Message};
use std::{io::Write, path::PathBuf};

use super::{Computer, ComputerError, FrameInfo, Result, digest, wall_ms};

impl Computer {
    pub(super) fn artifact_path(&self, session: &str, id: &str) -> Result<PathBuf> {
        if !id.starts_with("cuf_")
            || id.len() != 30
            || !id[4..].bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(ComputerError::FrameExpired);
        }
        Ok(self
            .artifacts
            .join(digest(session.as_bytes()))
            .join(format!("{id}.png")))
    }
    pub(super) fn store_frame(
        &self,
        frame: Frame,
        grant: String,
        capture: Capture,
    ) -> Result<FrameInfo> {
        let info = FrameInfo {
            id: frame.id,
            session_id: frame.session_id,
            grant_id: grant,
            captured_at: frame.captured_at,
            target: capture.target,
            sha256: digest(&capture.png),
            bytes: capture.png.len() as u64,
            created_ms: wall_ms(),
        };
        let path = self.artifact_path(&info.session_id, &info.id)?;
        let days = (self.policy)().retention_days;
        let expiry = (days != 0).then(|| {
            info.created_ms
                .saturating_add(days.saturating_mul(86_400_000).min(i64::MAX as u64) as i64)
        });
        // Reserve metadata before writing: a crash cannot leave an untracked
        // screenshot outside retention/accounting. A missing file is never an
        // input frame or a readable image receipt.
        self.journal.frame(&info, &info.target, expiry)?;
        let mut created = false;
        let written = (|| -> Result<()> {
            let directory = path.parent().ok_or(ComputerError::Storage)?;
            std::fs::create_dir_all(directory)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&self.artifacts, std::fs::Permissions::from_mode(0o700))?;
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
            }
            let mut options = std::fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&path)?;
            created = true;
            file.write_all(&capture.png)?;
            file.sync_all()?;
            let id = info.id.clone();
            self.journal.0.transaction(move |tx| {
                tx.execute(
                    "UPDATE computer_frame SET ready=1 WHERE id=?1 AND expired=0",
                    [id],
                )?;
                Ok(())
            })?;
            Ok(())
        })();
        if let Err(error) = written {
            let id = info.id.clone();
            let _ = self.journal.0.transaction(move |tx| {
                tx.execute("UPDATE computer_frame SET expired=1 WHERE id=?1", [id])?;
                Ok(())
            });
            if created {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
        Ok(info)
    }
    pub fn frame_bytes(&self, session: &str, id: &str) -> Result<Vec<u8>> {
        let (info, _) =
            self.journal
                .read_frame(session, id, wall_ms(), (self.policy)().retention_days)?;
        let path = self.artifact_path(session, id)?;
        let file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        if length > 8 * 1024 * 1024 || length != info.bytes {
            return Err(ComputerError::FrameExpired);
        }
        use std::io::Read;
        let mut bytes = Vec::with_capacity(length as usize);
        file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != info.bytes || digest(&bytes) != info.sha256 {
            return Err(ComputerError::FrameExpired);
        }
        Ok(bytes)
    }
    pub(super) fn prune(&self) -> Result<()> {
        let days = (self.policy)().retention_days;
        if days == 0 {
            return Ok(());
        }
        let cutoff =
            wall_ms().saturating_sub(days.saturating_mul(86_400_000).min(i64::MAX as u64) as i64);
        let mut cursor = String::new();
        loop {
            let after = cursor.clone();
            let stale=self.journal.0.transaction(move|tx| {
            let mut query=tx.prepare("SELECT id,session_id FROM computer_frame WHERE (expired=1 OR created_ms<=?1) AND id>?2 ORDER BY id LIMIT 500")?;
            let rows=query.query_map(rusqlite::params![cutoff,after],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            for (id,_) in &rows {tx.execute("UPDATE computer_frame SET expired=1 WHERE id=?1",[id])?;}Ok(rows)
        })?;
            let Some(last) = stale.last() else {
                break;
            };
            cursor = last.0.clone();
            for (id, session) in stale {
                if let Ok(path) = self.artifact_path(&session, &id) {
                    match std::fs::remove_file(path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => continue,
                    };
                    let _ = self.journal.0.transaction(move |tx| {
                        tx.execute("DELETE FROM computer_frame WHERE id=?1 AND expired=1", [id])?;
                        Ok(())
                    });
                }
            }
        }
        Ok(())
    }
    pub fn project(&self, session: &str, messages: &mut [Message], vision: bool) {
        for message in messages {
            for part in &mut message.content {
                let Content::Attachment(reference) = part else {
                    continue;
                };
                let Some(id) = reference
                    .name
                    .as_deref()
                    .and_then(|name| name.strip_prefix("computer:"))
                else {
                    continue;
                };
                let image = if (self.policy)().enabled && vision {
                    self.frame_bytes(session, id).ok()
                } else {
                    None
                };
                *part=match image {Some(bytes) if digest(&bytes)==reference.sha256=>Content::Image {media_type:"image/png".into(),data:base64::engine::general_purpose::STANDARD.encode(bytes)},_=>Content::Text {text:"[Private computer capture unavailable or expired; obtain a fresh authorized frame before acting.]".into()}};
            }
        }
    }
}
