//! The `system` route group (`observability-costs` → Machine stats): CPU, memory and the
//! Location's disk on the machine the server runs on, for the TUI sidebar. Never lists
//! processes, users or the host name.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{Json, Router, extract::State, http::request::Parts, routing::get};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, System};

use super::{
    ApiError, AppState,
    envelope::{Located, LocationInfo, location},
};

/// A cached sample is reused for this long; clients poll every few seconds.
const REFRESH: Duration = Duration::from_secs(2);
/// The disk list (mounts) is re-read this rarely; only the matched disk is refreshed.
const DISK_LIST_REFRESH: Duration = Duration::from_secs(60);
/// A JSON file holding a fixed [`Machine`] served instead of live readings, so the README
/// screenshots never show the capturing machine. Unset in normal use.
const FIXTURE_ENV: &str = "CYBER_MACHINE_FIXTURE";

/// One machine sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Machine {
    /// The OS name and version, such as `macOS 15.0`; never the host or user name.
    pub hostname_label: Option<String>,
    pub cores_logical: usize,
    pub cores_physical: Option<usize>,
    /// `null` until a second CPU sample exists: usage needs two readings.
    pub cpu_idle_percent: Option<f64>,
    /// 1, 5 and 15 minute load; `null` where the OS has none (Windows).
    pub load_avg: Option<[f64; 3]>,
    /// Bytes.
    pub memory_total: u64,
    /// Bytes.
    pub memory_available: u64,
    /// The volume holding the Location; `null` when no mount contains it.
    pub disk: Option<DiskUsage>,
    /// Unix milliseconds of this sample.
    pub sampled_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DiskUsage {
    pub mount: String,
    /// Bytes.
    pub total: u64,
    /// Bytes.
    pub available: u64,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/system", get(system))
}

async fn system(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Machine>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    if !directory.is_dir() {
        return Err(ApiError::invalid("System Location must be a directory"));
    }
    let machine = match fixture() {
        Some(machine) => Machine {
            sampled_at: unix_ms(),
            ..machine.clone()
        },
        None => {
            let probe = directory.clone();
            tokio::task::spawn_blocking(move || sample(&probe))
                .await
                .map_err(|_| ApiError::invalid("System sampling was interrupted"))?
        }
    };
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: machine,
    }))
}

/// The [`FIXTURE_ENV`] sample, read once; `None` when unset or unreadable.
fn fixture() -> Option<&'static Machine> {
    static FIXTURE: OnceLock<Option<Machine>> = OnceLock::new();
    FIXTURE
        .get_or_init(|| {
            let path = std::env::var_os(FIXTURE_ENV)?;
            parse_fixture(&std::fs::read_to_string(path).ok()?)
        })
        .as_ref()
}

fn parse_fixture(text: &str) -> Option<Machine> {
    serde_json::from_str(text).ok()
}

/// The process-wide sampler: one `System` refreshed at most every [`REFRESH`].
struct Sampler {
    system: System,
    disks: Disks,
    disks_listed: Option<Instant>,
    /// CPU usage was read once, so the next reading is a real delta.
    cpu_primed: bool,
    last: Option<(Instant, PathBuf, Machine)>,
    label: Option<String>,
    cores_physical: Option<usize>,
}

fn sampler() -> &'static Mutex<Sampler> {
    static SAMPLER: OnceLock<Mutex<Sampler>> = OnceLock::new();
    SAMPLER.get_or_init(|| {
        Mutex::new(Sampler {
            system: System::new(),
            disks: Disks::new(),
            disks_listed: None,
            cpu_primed: false,
            last: None,
            label: os_label(),
            cores_physical: System::physical_core_count(),
        })
    })
}

/// The latest sample for `directory`, reusing one younger than [`REFRESH`].
pub fn sample(directory: &Path) -> Machine {
    let mut sampler = sampler()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    if let Some((at, dir, machine)) = &sampler.last
        && now.duration_since(*at) < REFRESH
        && dir == directory
    {
        return machine.clone();
    }
    let machine = sampler.refresh(directory, now);
    sampler.last = Some((now, directory.to_path_buf(), machine.clone()));
    machine
}

impl Sampler {
    fn refresh(&mut self, directory: &Path, now: Instant) -> Machine {
        self.system
            .refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage());
        self.system
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let cpu_idle_percent = self
            .cpu_primed
            .then(|| (100.0 - f64::from(self.system.global_cpu_usage())).clamp(0.0, 100.0));
        self.cpu_primed = true;
        Machine {
            hostname_label: self.label.clone(),
            cores_logical: self.system.cpus().len(),
            cores_physical: self.cores_physical,
            cpu_idle_percent,
            load_avg: load_average(),
            memory_total: self.system.total_memory(),
            memory_available: self.system.available_memory(),
            disk: self.disk(directory, now),
            sampled_at: unix_ms(),
        }
    }

    /// The mount holding `directory`, refreshing only that disk's space.
    fn disk(&mut self, directory: &Path, now: Instant) -> Option<DiskUsage> {
        let stale = self
            .disks_listed
            .is_none_or(|at| now.duration_since(at) >= DISK_LIST_REFRESH);
        if stale {
            self.disks
                .refresh_specifics(true, DiskRefreshKind::nothing());
            self.disks_listed = Some(now);
        }
        let target = plain(&std::fs::canonicalize(directory).unwrap_or_else(|_| directory.into()));
        let mounts: Vec<PathBuf> = self
            .disks
            .list()
            .iter()
            .map(|d| d.mount_point().to_path_buf())
            .collect();
        let index = containing_mount(&target, &mounts);
        let mount = index.map_or_else(
            || target.display().to_string(),
            |i| mounts[i].display().to_string(),
        );
        // The Location's own file system: on macOS a firmlinked path such as /private/tmp
        // lives on the data volume, not the read-only root its longest mount prefix names.
        if let Some((total, available)) = statvfs(&target) {
            return Some(DiskUsage {
                mount,
                total,
                available,
            });
        }
        let disk = &mut self.disks.list_mut()[index?];
        disk.refresh_specifics(DiskRefreshKind::nothing().with_storage());
        Some(DiskUsage {
            mount,
            total: disk.total_space(),
            available: disk.available_space(),
        })
    }
}

/// The index of the longest mount point that contains `target`.
pub fn containing_mount(target: &Path, mounts: &[PathBuf]) -> Option<usize> {
    mounts
        .iter()
        .enumerate()
        .filter(|(_, mount)| contains(mount, target))
        .max_by_key(|(_, mount)| mount.components().count())
        .map(|(index, _)| index)
}

fn contains(mount: &Path, target: &Path) -> bool {
    if cfg!(windows) {
        let lower = |p: &Path| p.to_string_lossy().to_lowercase().replace('/', "\\");
        let (mount, target) = (lower(mount), lower(target));
        let mount = mount.trim_end_matches('\\');
        target == mount || target.starts_with(&format!("{mount}\\"))
    } else {
        target.starts_with(mount)
    }
}

/// A Windows verbatim path (`\\?\C:\…`) in the plain form mount points use.
fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// `(total, available to unprivileged users)` bytes of the file system holding `path`.
#[cfg(unix)]
fn statvfs(path: &Path) -> Option<(u64, u64)> {
    let stats = rustix::fs::statvfs(path).ok()?;
    let block = stats.f_frsize;
    Some((
        stats.f_blocks.saturating_mul(block),
        stats.f_bavail.saturating_mul(block),
    ))
}

#[cfg(not(unix))]
fn statvfs(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn load_average() -> Option<[f64; 3]> {
    if cfg!(windows) {
        return None;
    }
    let load = System::load_average();
    Some([load.one, load.five, load.fifteen])
}

/// `macOS 15.0`: the OS, never the host name.
fn os_label() -> Option<String> {
    let name = match System::name()? {
        name if name == "Darwin" => "macOS".to_owned(),
        name => name,
    };
    Some(match System::os_version() {
        Some(version) => format!("{name} {version}"),
        None => name,
    })
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_containing_mount_wins() {
        let mounts: Vec<PathBuf> = ["/", "/home", "/home/me/data", "/homework"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let found = |p: &str| containing_mount(Path::new(p), &mounts).map(|i| &mounts[i]);
        assert_eq!(found("/home/me/repo"), Some(&PathBuf::from("/home")));
        assert_eq!(
            found("/home/me/data/x"),
            Some(&PathBuf::from("/home/me/data"))
        );
        assert_eq!(found("/homework/a"), Some(&PathBuf::from("/homework")));
        assert_eq!(found("/etc"), Some(&PathBuf::from("/")));
        assert_eq!(containing_mount(Path::new("/x"), &[]), None);
    }

    #[test]
    fn a_fixture_parses_and_a_malformed_one_is_ignored() {
        let machine = parse_fixture(
            r#"{"hostname_label":"macOS 15.0","cores_logical":12,"cores_physical":12,
                "cpu_idle_percent":77.0,"load_avg":[2.1,1.8,1.6],"memory_total":34359738368,
                "memory_available":19112603648,"disk":{"mount":"/","total":926000000000,
                "available":514000000000},"sampled_at":0}"#,
        )
        .expect("fixture");
        assert_eq!(machine.cores_logical, 12);
        assert_eq!(machine.disk.map(|d| d.available), Some(514_000_000_000));
        assert_eq!(parse_fixture(r#"{"cores_logical":"twelve"}"#), None);
    }

    #[test]
    fn verbatim_paths_compare_in_plain_form() {
        assert_eq!(plain(Path::new(r"\\?\C:\work")), PathBuf::from(r"C:\work"));
        assert_eq!(plain(Path::new("/work")), PathBuf::from("/work"));
    }
}
