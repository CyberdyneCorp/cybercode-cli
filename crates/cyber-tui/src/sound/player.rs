//! The platform's own audio player (`tui` → Sound alerts): found once on `PATH`, run
//! directly and never through a shell, with no access to the terminal. One plays at a
//! time; it is killed after [`KILL_AFTER`], when a newer sound starts and when the TUI
//! exits. Without a player, or over SSH, alerts ring the terminal bell instead.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::Job;

/// A player still running after this is killed.
pub const KILL_AFTER: Duration = Duration::from_secs(5);

/// The supported players.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Afplay,
    PwPlay,
    Paplay,
    Aplay,
    PowerShell,
}

impl Kind {
    /// The executable searched for on `PATH`.
    pub fn program(self) -> &'static str {
        match self {
            Kind::Afplay => "afplay",
            Kind::PwPlay => "pw-play",
            Kind::Paplay => "paplay",
            Kind::Aplay => "aplay",
            Kind::PowerShell => "powershell.exe",
        }
    }

    /// The arguments that play `file`, and the environment variable that carries it on
    /// Windows, where the path is never part of the command string.
    pub fn argv(self, file: &Path) -> (Vec<OsString>, Option<(&'static str, OsString)>) {
        let file = file.as_os_str().to_owned();
        match self {
            Kind::Afplay | Kind::PwPlay | Kind::Paplay => (vec![file], None),
            Kind::Aplay => (vec!["-q".into(), file], None),
            Kind::PowerShell => (
                [
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "(New-Object Media.SoundPlayer $env:CYBER_SOUND_FILE).PlaySync()",
                ]
                .map(OsString::from)
                .to_vec(),
                Some(("CYBER_SOUND_FILE", file)),
            ),
        }
    }

    /// Whether this player can play the custom file `file`, by its extension:
    /// `System.Media.SoundPlayer` and `aplay` play WAV only (`aplay` would play anything
    /// else as noise), `afplay` has no Ogg decoder.
    pub fn plays(self, file: &Path) -> bool {
        let extension = file
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match self {
            Kind::PowerShell | Kind::Aplay => extension == "wav",
            Kind::Afplay => !matches!(extension.as_str(), "ogg" | "oga"),
            Kind::PwPlay | Kind::Paplay => true,
        }
    }
}

/// The operating systems with a player list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    /// Linux and the BSDs.
    Unix,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Unix
        }
    }

    /// The players to look for, in order.
    pub fn players(self) -> &'static [Kind] {
        match self {
            Os::Mac => &[Kind::Afplay],
            Os::Windows => &[Kind::PowerShell],
            Os::Unix => &[Kind::PwPlay, Kind::Paplay, Kind::Aplay],
        }
    }
}

/// Why the terminal bell plays instead of a sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BellReason {
    NoPlayer,
    /// `SSH_CONNECTION` or `SSH_TTY` is set without `play_over_ssh`: a sound would play
    /// on the remote machine.
    Ssh,
    /// The player could not be started or failed.
    Failed,
    /// No cache directory to render the sounds into.
    NoCache,
    /// The pack is `bell`.
    Pack,
}

impl BellReason {
    pub fn describe(self) -> &'static str {
        match self {
            BellReason::NoPlayer => "no player found",
            BellReason::Ssh => "SSH session",
            BellReason::Failed => "the player failed",
            BellReason::NoCache => "no cache directory",
            BellReason::Pack => "bell pack",
        }
    }
}

/// Where alert sounds go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Player { kind: Kind, program: PathBuf },
    Bell(BellReason),
}

/// The output for this environment: the bell over SSH (unless `play_over_ssh`) or
/// without a cache directory, else the first player of this OS on `PATH`.
pub fn output(env: &dyn Fn(&str) -> Option<String>, play_over_ssh: bool, cache: bool) -> Output {
    let ssh = env("SSH_CONNECTION").is_some() || env("SSH_TTY").is_some();
    if ssh && !play_over_ssh {
        return Output::Bell(BellReason::Ssh);
    }
    if !cache {
        return Output::Bell(BellReason::NoCache);
    }
    let path = env("PATH").unwrap_or_default();
    match find(Os::current(), &path) {
        Some((kind, program)) => Output::Player { kind, program },
        None => Output::Bell(BellReason::NoPlayer),
    }
}

/// The first of `os`'s players in the `PATH` value `path`. Relative entries (`.`,
/// `node_modules/.bin`) are skipped: they would resolve against the project directory,
/// and a repository could ship its own "player".
pub fn find(os: Os, path: &str) -> Option<(Kind, PathBuf)> {
    os.players().iter().find_map(|&kind| {
        std::env::split_paths(path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(kind.program()))
            .find(|candidate| executable(candidate))
            .map(|program| (kind, program))
    })
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    meta.is_file()
}

/// Why a sound did not play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The player could not be started, or the pack sound not rendered: later alerts
    /// ring the bell.
    Start(String),
    /// The player exited with an error playing `file` (a custom file), or a pack sound.
    Exit {
        file: Option<PathBuf>,
        error: String,
    },
}

/// Plays jobs. A failure to play is sent on `failed`.
pub trait Player: Send {
    /// Play `job`, stopping a sound still playing. Never blocks.
    fn play(&mut self, job: Job, failed: &Sender<Failure>);
    fn stop(&mut self);
    /// A player process is still running (tests).
    #[cfg(test)]
    fn busy(&self) -> bool {
        false
    }
}

/// No player: the bell rings instead.
pub struct Nothing;

impl Player for Nothing {
    fn play(&mut self, _: Job, _: &Sender<Failure>) {}
    fn stop(&mut self) {}
}

/// The running player and the play it belongs to.
#[derive(Default)]
struct Playing {
    generation: u64,
    child: Option<Child>,
}

/// A player process per sound.
pub struct Process {
    kind: Kind,
    program: PathBuf,
    playing: Arc<Mutex<Playing>>,
    /// [`KILL_AFTER`], shorter in tests.
    kill_after: Duration,
}

impl Process {
    pub fn new(kind: Kind, program: &Path) -> Self {
        Self {
            kind,
            program: program.to_path_buf(),
            playing: Default::default(),
            kill_after: KILL_AFTER,
        }
    }
}

fn lock(playing: &Mutex<Playing>) -> std::sync::MutexGuard<'_, Playing> {
    playing.lock().unwrap_or_else(PoisonError::into_inner)
}

fn kill(child: &mut Option<Child>) {
    if let Some(mut child) = child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The player's command: stdio null, and on Unix its own process group, so Ctrl+C in
/// the TUI does not reach it; on Windows no console.
fn command(kind: Kind, program: &Path, file: &Path) -> Command {
    let (args, env) = kind.argv(file);
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some((name, value)) = env {
        command.env(name, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

impl Player for Process {
    /// The file is rendered and the player started on a thread, which then waits for
    /// it: the UI never waits on either.
    fn play(&mut self, job: Job, failed: &Sender<Failure>) {
        let generation = {
            let mut playing = lock(&self.playing);
            kill(&mut playing.child);
            playing.generation += 1;
            playing.generation
        };
        let (kind, program, kill_after) = (self.kind, self.program.clone(), self.kill_after);
        let (playing, failed) = (self.playing.clone(), failed.clone());
        std::thread::spawn(move || {
            let started = job.prepare().and_then(|file| {
                let mut current = lock(&playing);
                if current.generation != generation {
                    return Ok(false);
                }
                current.child = Some(command(kind, &program, &file).spawn()?);
                Ok(true)
            });
            let error = match started {
                Ok(true) => watch(&playing, generation, kill_after),
                Ok(false) => return,
                Err(error) => {
                    let _ = failed.send(Failure::Start(format!("{}: {error}", kind.program())));
                    return;
                }
            };
            if let Some(status) = error {
                let file = match job {
                    Job::File(path) => Some(path),
                    Job::Render { .. } => None,
                };
                let error = format!("{} exited with {status}", kind.program());
                let _ = failed.send(Failure::Exit { file, error });
            }
        });
    }

    fn stop(&mut self) {
        let mut playing = lock(&self.playing);
        playing.generation += 1;
        kill(&mut playing.child);
    }

    #[cfg(test)]
    fn busy(&self) -> bool {
        lock(&self.playing).child.is_some()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Wait for the player of `generation`; it is killed after `kill_after`. A newer sound
/// or [`Player::stop`] ends the wait. The failing exit status, if it failed.
fn watch(
    playing: &Mutex<Playing>,
    generation: u64,
    kill_after: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + kill_after;
    loop {
        std::thread::sleep(Duration::from_millis(20));
        let mut current = lock(playing);
        if current.generation != generation {
            return None;
        }
        let child = current.child.as_mut()?;
        match child.try_wait() {
            Ok(Some(status)) => {
                current.child = None;
                return (!status.success()).then_some(status);
            }
            Ok(None) if Instant::now() < deadline => {}
            _ => {
                kill(&mut current.child);
                return None;
            }
        }
    }
}

/// A player that records what it was asked to play (tests): nothing is spawned.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct Recorder {
    played: Arc<Mutex<Vec<PathBuf>>>,
    /// Every play fails, as a player that cannot start.
    pub fail: bool,
    /// Every play is recorded, then the player exits with an error.
    pub exit_error: bool,
}

#[cfg(test)]
impl Recorder {
    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }

    pub fn exiting_with_error() -> Self {
        Self {
            exit_error: true,
            ..Self::default()
        }
    }

    pub fn played(&self) -> Vec<PathBuf> {
        self.played
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
impl Player for Recorder {
    fn play(&mut self, job: Job, failed: &Sender<Failure>) {
        if self.fail {
            let _ = failed.send(Failure::Start("recorder: cannot start".into()));
            return;
        }
        self.played
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(job.path());
        if self.exit_error {
            let file = match job {
                Job::File(path) => Some(path),
                Job::Render { .. } => None,
            };
            let error = "recorder exited with exit status: 1".into();
            let _ = failed.send(Failure::Exit { file, error });
        }
    }

    fn stop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, String)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone())
        }
    }

    /// An executable file named `name` in `dir`.
    fn stub(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn path_of(dirs: &[&Path]) -> String {
        std::env::join_paths(dirs).unwrap().into_string().unwrap()
    }

    #[test]
    fn players_are_found_in_os_order_then_path_order() {
        let root = tempfile::tempdir().unwrap();
        let (first, second) = (root.path().join("a"), root.path().join("b"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        stub(&second, "aplay");
        let path = path_of(&[&first, &second]);
        assert_eq!(
            find(Os::Unix, &path),
            Some((Kind::Aplay, second.join("aplay")))
        );
        // pw-play comes before aplay whichever directory holds it.
        let pw = stub(&second, "pw-play");
        stub(&first, "paplay");
        assert_eq!(find(Os::Unix, &path), Some((Kind::PwPlay, pw)));
        // The first directory wins for the same player.
        let early = stub(&first, "pw-play");
        assert_eq!(find(Os::Unix, &path), Some((Kind::PwPlay, early)));
        assert_eq!(find(Os::Mac, &path), None);
        let afplay = stub(&second, "afplay");
        assert_eq!(find(Os::Mac, &path), Some((Kind::Afplay, afplay)));
        let powershell = stub(&first, "powershell.exe");
        assert_eq!(
            find(Os::Windows, &path),
            Some((Kind::PowerShell, powershell))
        );
        assert_eq!(find(Os::Unix, ""), None);
    }

    /// A non-executable file of the right name is not a player.
    #[cfg(unix)]
    #[test]
    fn a_file_without_execute_permission_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("aplay"), "").unwrap();
        assert_eq!(find(Os::Unix, &path_of(&[root.path()])), None);
    }

    /// The Windows command string never holds the path: it travels in
    /// `CYBER_SOUND_FILE`, so a file name cannot inject PowerShell.
    #[test]
    fn the_windows_command_never_contains_the_path() {
        let file = Path::new("C:\\Users\\x\\a'; Remove-Item -Recurse ~; '.wav");
        let (args, env) = Kind::PowerShell.argv(file);
        assert!(
            args.iter()
                .all(|arg| !arg.to_string_lossy().contains("Remove-Item"))
        );
        assert_eq!(env, Some(("CYBER_SOUND_FILE", file.as_os_str().to_owned())));
        assert_eq!(
            args[3],
            "(New-Object Media.SoundPlayer $env:CYBER_SOUND_FILE).PlaySync()"
        );
        let (args, env) = Kind::Aplay.argv(Path::new("x.wav"));
        assert_eq!(args, ["-q", "x.wav"]);
        assert_eq!(env, None);
        assert_eq!(Kind::Afplay.argv(Path::new("x.wav")).0, ["x.wav"]);
    }

    #[test]
    fn ssh_without_play_over_ssh_chooses_the_bell() {
        let root = tempfile::tempdir().unwrap();
        for name in ["afplay", "pw-play", "powershell.exe"] {
            stub(root.path(), name);
        }
        let path = path_of(&[root.path()]);
        for var in ["SSH_CONNECTION", "SSH_TTY"] {
            let vars = [(var, "1 2 3 4".to_owned()), ("PATH", path.clone())];
            assert_eq!(
                output(&env(&vars), false, true),
                Output::Bell(BellReason::Ssh)
            );
            assert!(matches!(
                output(&env(&vars), true, true),
                Output::Player { .. }
            ));
        }
        let vars = [("PATH", path)];
        assert!(matches!(
            output(&env(&vars), false, true),
            Output::Player { .. }
        ));
        assert_eq!(
            output(&env(&vars), false, false),
            Output::Bell(BellReason::NoCache)
        );
        let empty = tempfile::tempdir().unwrap();
        let vars = [("PATH", path_of(&[empty.path()]))];
        assert_eq!(
            output(&env(&vars), false, true),
            Output::Bell(BellReason::NoPlayer)
        );
    }

    /// A player that cannot be spawned reports it; the rendered file was written first.
    #[test]
    fn a_spawn_error_is_reported() {
        let cache = tempfile::tempdir().unwrap();
        let mut player = Process::new(Kind::Afplay, &cache.path().join("missing-player"));
        let (tx, rx) = std::sync::mpsc::channel();
        let job = Job::Render {
            dir: cache.path().to_path_buf(),
            pack: super::super::Pack::Soft,
            event: super::super::Alert::Message,
            volume: 10,
        };
        player.play(job.clone(), &tx);
        let Failure::Start(error) = rx.recv_timeout(Duration::from_secs(10)).unwrap() else {
            panic!("a start failure");
        };
        assert!(error.starts_with("afplay: "), "{error}");
        assert!(job.path().is_file());
    }

    /// Relative `PATH` entries resolve against the working directory, usually the
    /// project checkout: never searched, even when one leads to a player.
    #[cfg(unix)]
    #[test]
    fn relative_path_entries_are_skipped() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        stub(&bin, "aplay");
        // `bin` as seen from the working directory: `../../…/tmp/…/bin`.
        let cwd = std::env::current_dir().unwrap();
        let up = cwd.components().count() - 1;
        let relative: PathBuf = std::iter::repeat_n(Path::new(".."), up)
            .collect::<PathBuf>()
            .join(bin.strip_prefix("/").unwrap());
        assert!(
            relative.join("aplay").is_file(),
            "the entry leads to the stub"
        );
        assert_eq!(find(Os::Unix, &path_of(&[&relative])), None);
        assert_eq!(
            find(Os::Unix, &path_of(&[&relative, &bin])),
            Some((Kind::Aplay, bin.join("aplay")))
        );
    }

    #[test]
    fn players_take_only_the_custom_formats_they_decode() {
        for (kind, file, plays) in [
            (Kind::PowerShell, "a.wav", true),
            (Kind::PowerShell, "a.mp3", false),
            (Kind::Aplay, "a.WAV", true),
            (Kind::Aplay, "a.ogg", false),
            (Kind::Afplay, "a.mp3", true),
            (Kind::Afplay, "a.ogg", false),
            (Kind::PwPlay, "a.oga", true),
            (Kind::Paplay, "a.aiff", true),
        ] {
            assert_eq!(kind.plays(Path::new(file)), plays, "{kind:?} {file}");
        }
    }

    /// A stub player that runs until killed, and a custom WAV for it to "play".
    #[cfg(unix)]
    fn sleeper(dir: &Path) -> (Process, Job) {
        let program = dir.join("pw-play");
        std::fs::write(&program, "#!/bin/sh\nexec sleep 30\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = dir.join("a.wav");
        std::fs::write(&file, b"RIFF").unwrap();
        (Process::new(Kind::PwPlay, &program), Job::File(file))
    }

    #[cfg(unix)]
    fn pid(player: &Process) -> Option<u32> {
        lock(&player.playing).child.as_ref().map(Child::id)
    }

    #[cfg(unix)]
    fn wait_for_child(player: &Process) -> u32 {
        for _ in 0..500 {
            if let Some(pid) = pid(player) {
                return pid;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the player did not start");
    }

    /// Whether process `pid` is still running (a reaped child no longer exists).
    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// A newer sound kills the playing one, and `stop` kills the last.
    #[cfg(unix)]
    #[test]
    fn a_newer_sound_and_stop_kill_the_player() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, job) = sleeper(dir.path());
        let (tx, rx) = std::sync::mpsc::channel();
        player.play(job.clone(), &tx);
        let first = wait_for_child(&player);
        player.play(job, &tx);
        assert!(!alive(first), "the first player was killed");
        let second = wait_for_child(&player);
        assert_ne!(first, second);
        assert!(player.busy());
        player.stop();
        assert!(!player.busy());
        assert!(!alive(second), "stop killed the player");
        std::thread::sleep(Duration::from_millis(100));
        assert!(rx.try_recv().is_err(), "a killed player is no failure");
    }

    /// A player still running after the time limit is killed, and that is no failure.
    #[cfg(unix)]
    #[test]
    fn a_player_running_too_long_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, job) = sleeper(dir.path());
        player.kill_after = Duration::from_millis(200);
        let (tx, rx) = std::sync::mpsc::channel();
        player.play(job, &tx);
        let pid = wait_for_child(&player);
        for _ in 0..500 {
            if !player.busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!player.busy());
        assert!(!alive(pid));
        assert!(rx.try_recv().is_err());
    }

    /// A custom file the player exits on is reported with the file, so only that
    /// event falls back; the player is not given up.
    #[cfg(unix)]
    #[test]
    fn a_failing_exit_names_the_custom_file() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("pw-play");
        std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = dir.path().join("a.mp3");
        let mut player = Process::new(Kind::PwPlay, &program);
        let (tx, rx) = std::sync::mpsc::channel();
        player.play(Job::File(file.clone()), &tx);
        let failure = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let Failure::Exit {
            file: Some(failed),
            error,
        } = failure
        else {
            panic!("{failure:?}");
        };
        assert_eq!(failed, file);
        assert!(error.starts_with("pw-play exited with"), "{error}");
    }
}
