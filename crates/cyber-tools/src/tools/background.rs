//! `background: true` for `bash` and `powershell` (`background-tasks` → Background bash,
//! Reading job output, Stop kills process trees). The command is authorized and prepared
//! exactly as in the foreground; the runtime then owns it as a process Job.

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cyber_server::runtime::{
    Job, JobIo, ProcessJob, ProcessReservation, ProcessWork, Runtime, RuntimeError,
};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use super::process::{Process, ReadStream};
use super::{ToolError, failed, text};
use crate::host::Ctx;
use crate::sandboxing::{NetworkAsk, Prepared};

const DEFAULT_MAX_JOBS: u64 = 20;
const DEFAULT_MAX_OUTPUT_MB: u64 = 64;
/// How long a stopped Job's processes get after SIGTERM before SIGKILL.
#[cfg(unix)]
const GRACE: Duration = Duration::from_secs(5);

/// Whether the call asks for a background Job; `notify` only applies to one.
pub(super) fn requested(input: &Value) -> Result<bool, ToolError> {
    let flag = |key: &str| match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(other) => Err(failed(format!("{key} must be a boolean, got {other}"))),
    };
    let background = flag("background")?.unwrap_or(false);
    if flag("notify")?.is_some() && !background {
        return Err(failed("notify applies only with background: true"));
    }
    Ok(background)
}

/// `background.max_jobs` and `background.max_output_mb` of a Location.
struct Settings {
    max_jobs: usize,
    max_output_bytes: u64,
}

impl Settings {
    fn of(config: &Value) -> Result<Self, ToolError> {
        let positive = |key: &str, default: u64| match config.pointer(&format!("/background/{key}"))
        {
            None | Some(Value::Null) => Ok(default),
            Some(value) => value
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or_else(|| failed(format!("background.{key} must be a positive integer"))),
        };
        let max_jobs = positive("max_jobs", DEFAULT_MAX_JOBS)?;
        let max_output_mb = positive("max_output_mb", DEFAULT_MAX_OUTPUT_MB)?;
        Ok(Self {
            max_jobs: usize::try_from(max_jobs).unwrap_or(usize::MAX),
            max_output_bytes: max_output_mb.saturating_mul(1024 * 1024),
        })
    }
}

/// Where a Job's output goes: its capped log and, once the Job runs, live
/// `job.output.1`.
pub(super) struct Sink {
    log: JobLog,
    io: Option<Arc<JobIo>>,
}

pub(super) type SharedSink = Arc<Mutex<Sink>>;

impl Sink {
    /// Start publishing live output through the running Job's `io`.
    pub(super) fn attach(sink: &SharedSink, io: JobIo) -> Arc<JobIo> {
        let io = Arc::new(io);
        sink.lock().unwrap_or_else(PoisonError::into_inner).io = Some(Arc::clone(&io));
        io
    }

    pub(super) fn write(&mut self, chunk: &[u8]) {
        self.log.write(chunk);
        if let Some(io) = &self.io {
            io.output(String::from_utf8_lossy(chunk).into_owned());
        }
    }
}

/// An admitted process Job and its log, before its process is handed over.
pub(super) struct Admitted {
    runtime: Runtime,
    reservation: ProcessReservation,
    pub sink: SharedSink,
}

impl Admitted {
    /// The ID the Job will have.
    #[cfg(unix)]
    pub(super) fn id(&self) -> &str {
        self.reservation.id()
    }
}

/// Reserve a process Job of `kind` for the call's Session and create its log.
pub(super) async fn admit(ctx: &Ctx<'_>, command: &str, kind: &str) -> Result<Admitted, ToolError> {
    let runtime = ctx
        .host
        .runtime()
        .ok_or_else(|| failed("Background jobs require the runtime"))?;
    let settings = Settings::of(&ctx.host.config_for(&ctx.location))?;
    let data = ctx
        .host
        .opts
        .tool_output_dir
        .parent()
        .ok_or_else(|| failed("Missing the data directory for job logs"))?;
    let input = &ctx.inv.input;
    let reservation = runtime
        .reserve_process_job(ProcessJob {
            session_id: ctx.inv.session_id.clone(),
            kind: kind.into(),
            command: command.into(),
            description: text(input, "description").into(),
            notify: input.get("notify").and_then(Value::as_bool) != Some(false),
            output_dir: data.join("jobs"),
            max_jobs: settings.max_jobs,
        })
        .await
        .map_err(refusal)?;
    let log = JobLog::create(reservation.output_path(), settings.max_output_bytes)
        .map_err(|e| failed(format!("Could not create the job log: {e}")))?;
    Ok(Admitted {
        runtime,
        reservation,
        sink: Arc::new(Mutex::new(Sink { log, io: None })),
    })
}

/// A running process the Job will own; its `readers` already feed the Job's sink.
pub(super) struct Owned {
    pub process: Process,
    pub prepared: Prepared,
    pub readers: Vec<tokio::task::JoinHandle<()>>,
}

/// Start `prepared` as a process Job of `kind` and describe it for the model at once.
pub(super) async fn start(
    ctx: &Ctx<'_>,
    prepared: Prepared,
    workdir: &Path,
    command: &str,
    kind: &str,
) -> Result<String, ToolError> {
    start_job(ctx, prepared, workdir, command, kind)
        .await
        .map(|job| describe(&job))
}

/// Start `prepared` as a process Job of `kind`.
pub(super) async fn start_job(
    ctx: &Ctx<'_>,
    prepared: Prepared,
    workdir: &Path,
    command: &str,
    kind: &str,
) -> Result<Job, ToolError> {
    let admitted = admit(ctx, command, kind).await?;
    let mut process = super::bash::spawn(ctx, &prepared, workdir, Stdio::piped()).await?;
    let readers = [process.stdout(), process.stderr()]
        .into_iter()
        .flatten()
        .map(|stream| tokio::spawn(pump(stream, Arc::clone(&admitted.sink))))
        .collect();
    let owned = Owned {
        process,
        prepared,
        readers,
    };
    launch(admitted, owned).await
}

/// Hand `owned` to the runtime as the admitted Job.
pub(super) async fn launch(admitted: Admitted, mut owned: Owned) -> Result<Job, ToolError> {
    let asks = owned.prepared.asks.take();
    launch_with(admitted, move |io, sink| {
        Box::pin(run(owned, asks, sink, io))
    })
    .await
}

/// Start the admitted Job with `work`, which gets the Job's `io` and log.
pub(super) async fn launch_with(
    admitted: Admitted,
    work: impl FnOnce(JobIo, SharedSink) -> ProcessWork + Send + 'static,
) -> Result<Job, ToolError> {
    let Admitted {
        runtime,
        reservation,
        sink,
    } = admitted;
    runtime
        .start_process_job(reservation, Box::new(move |io| work(io, sink)))
        .await
        .map_err(refusal)
}

/// The tool result for a started Job.
pub(super) fn describe(job: &Job) -> String {
    json!({
        "job_id": job.id,
        "status": job.status,
        "output_path": job.output_path,
        "message": "The command runs in the background. Read output_path to see its output; task_stop stops it. Do not wait or poll for it: a completion notice arrives as a queued message unless notify was false.",
    })
    .to_string()
}

pub(super) fn refusal(error: RuntimeError) -> ToolError {
    match error {
        RuntimeError::Invalid(message) => failed(message),
        other => failed(other.to_string()),
    }
}

/// The owned process until it exits or the Job is stopped. Network questions from the
/// sandbox proxy are refused: no person can answer them once the Turn has moved on.
/// `owned.prepared` keeps the proxy alive.
async fn run(
    mut owned: Owned,
    mut asks: Option<tokio::sync::mpsc::Receiver<NetworkAsk>>,
    sink: SharedSink,
    io: JobIo,
) -> Result<Option<i32>, String> {
    let stop = Sink::attach(&sink, io).stop.clone();
    let process = &mut owned.process;
    let status = loop {
        tokio::select! {
            status = process.wait() => break Some(status),
            _ = stop.cancelled() => break None,
            Some((_, reply)) = super::bash::next_ask(&mut asks) => { let _ = reply.send(false); }
        }
    };
    let code = match status {
        Some(status) => status.map_err(|e| e.to_string())?.code(),
        None => {
            stop_tree(process).await;
            None
        }
    };
    for reader in owned.readers {
        // Grandchildren may hold the pipes open; do not wait on them forever.
        let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
    }
    Ok(code)
}

/// SIGTERM to the whole process group, up to 5 s for the leader to exit, then SIGKILL to
/// what remains. On Windows the Job Object is terminated.
pub(super) async fn stop_tree(process: &mut Process) {
    #[cfg(unix)]
    if process.request_graceful_stop().unwrap_or(false) {
        let deadline = tokio::time::Instant::now() + GRACE;
        while tokio::time::Instant::now() < deadline && !process.leader_exited().unwrap_or(true) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    process.terminate();
    let _ = process.wait().await;
}

async fn pump(mut stream: ReadStream, sink: SharedSink) {
    let mut buf = vec![0_u8; 8192];
    while let Ok(n) = stream.read(&mut buf).await {
        if n == 0 {
            break;
        }
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write(&buf[..n]);
    }
}

/// `<data>/jobs/<job_id>.log`, capped at `background.max_output_mb`: past the cap the
/// oldest half is dropped and a marker line says how many bytes are gone.
pub(super) struct JobLog {
    file: std::fs::File,
    path: PathBuf,
    len: u64,
    cap: u64,
    dropped: u64,
}

impl JobLog {
    pub(super) fn create(path: &Path, cap: u64) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            len: 0,
            cap,
            dropped: 0,
        })
    }

    pub(super) fn write(&mut self, chunk: &[u8]) {
        if self.file.write_all(chunk).is_ok() {
            self.len += chunk.len() as u64;
        }
        if self.len > self.cap
            && let Err(error) = self.compact()
        {
            cyber_core::log::error(
                "background",
                &format!("Could not truncate {}: {error}", self.path.display()),
                json!({}),
            );
        }
    }

    /// Keep the newest half of the cap, from a line start when one is near.
    fn compact(&mut self) -> std::io::Result<()> {
        let keep = self.cap / 2;
        let mut tail = Vec::new();
        self.file.seek(SeekFrom::Start(self.len - keep))?;
        self.file.read_to_end(&mut tail)?;
        let start = tail
            .iter()
            .take(4096)
            .position(|byte| *byte == b'\n')
            .map_or(0, |i| i + 1);
        let kept = &tail[start..];
        // Everything before the kept bytes is dropped output, except the old marker.
        let marker_len = self.marker(self.dropped).len() as u64;
        let earlier = if self.dropped > 0 { marker_len } else { 0 };
        self.dropped += self.len - earlier - kept.len() as u64;
        let marker = self.marker(self.dropped);
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(marker.as_bytes())?;
        self.file.write_all(kept)?;
        self.len = (marker.len() + kept.len()) as u64;
        Ok(())
    }

    fn marker(&self, dropped: u64) -> String {
        format!(
            "[output truncated: {dropped} earlier bytes dropped; background.max_output_mb is {}]\n",
            self.cap / (1024 * 1024)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_needs_background_and_flags_must_be_booleans() {
        assert!(!requested(&json!({})).unwrap());
        assert!(requested(&json!({"background": true, "notify": false})).unwrap());
        assert!(requested(&json!({"notify": false})).is_err());
        assert!(requested(&json!({"background": "yes"})).is_err());
    }

    #[test]
    fn settings_default_and_refuse_non_positive_values() {
        let settings = Settings::of(&json!({})).unwrap();
        assert_eq!(settings.max_jobs, 20);
        assert_eq!(settings.max_output_bytes, 64 * 1024 * 1024);
        assert!(Settings::of(&json!({"background": {"max_jobs": 0}})).is_err());
        assert!(Settings::of(&json!({"background": {"max_output_mb": "big"}})).is_err());
    }

    #[test]
    fn repeated_compaction_counts_dropped_output_bytes_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.log");
        let mut log = JobLog::create(&path, 1024 * 1024).unwrap();
        let mut line = vec![b'x'; 99];
        line.push(b'\n');
        let total = 30_000;
        for _ in 0..total {
            log.write(&line);
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let (marker, kept) = content.split_once('\n').unwrap();
        let dropped: u64 = marker
            .strip_prefix("[output truncated: ")
            .and_then(|rest| rest.split(' ').next())
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(dropped + kept.len() as u64, total * 100);
        assert!(content.len() as u64 <= 1024 * 1024);
        assert!(kept.starts_with('x'));
    }
}
