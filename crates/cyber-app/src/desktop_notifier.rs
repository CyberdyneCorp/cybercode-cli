//! The OS desktop notifier of scheduled runs (`loops-scheduling` → Completion
//! notifications): a scheduled run usually has no client attached to show its
//! notification, so the service shows it. macOS uses `osascript`, Linux `notify-send`;
//! other platforms have none. Title and body are passed as arguments, never inlined in a
//! script or a shell command line. Best effort: a missing tool or a failure is ignored,
//! and a notifier that hangs is killed after a few seconds.

use std::process::Stdio;
use std::time::Duration;

use cyber_server::runtime::schedules::DesktopNotifier;

/// How long a notifier process may run.
const TIMEOUT: Duration = Duration::from_secs(5);
/// Longest title and body passed on.
const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 400;

pub struct OsNotifier;

/// The program and arguments that show `title` and `body`, or `None` on a platform
/// without a notifier.
pub fn command(title: &str, body: &str) -> Option<(&'static str, Vec<String>)> {
    let title: String = title.chars().take(MAX_TITLE).collect();
    let body: String = body.chars().take(MAX_BODY).collect();
    if cfg!(target_os = "macos") {
        // The text arrives as `argv`, so no quoting can turn it into AppleScript.
        let script = [
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
        ];
        let mut args: Vec<String> = script.iter().map(|s| (*s).to_owned()).collect();
        args.extend([title, body]);
        return Some(("osascript", args));
    }
    if cfg!(target_os = "linux") {
        return Some(("notify-send", vec!["--".into(), title, body]));
    }
    None
}

impl DesktopNotifier for OsNotifier {
    fn notify(&self, title: &str, body: &str) {
        let Some((program, args)) = command(title, body) else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            let child = tokio::process::Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn();
            if let Ok(mut child) = child {
                let _ = tokio::time::timeout(TIMEOUT, child.wait()).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_passed_as_arguments_only() {
        let hostile = "\" & do shell script \"touch /tmp/x\" & \"";
        let Some((program, args)) = command("Schedule nightly", hostile) else {
            return;
        };
        assert!(matches!(program, "osascript" | "notify-send"));
        assert_eq!(args.last().map(String::as_str), Some(hostile));
        assert!(
            args.iter()
                .filter(|a| a.as_str() != hostile)
                .all(|a| !a.contains("touch")),
            "{args:?}"
        );
        let long = "x".repeat(1000);
        let (_, args) = command(&long, &long).unwrap();
        assert!(args.iter().all(|a| a.chars().count() <= MAX_BODY));
    }
}
