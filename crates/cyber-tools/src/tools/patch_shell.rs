//! `apply_patch` sent through the shell (`builtin-tools` → apply_patch through the
//! shell): a `bash` or `powershell` script that is nothing but an `apply_patch` heredoc,
//! optionally after `cd <dir> &&`, runs as the patch tool when no `apply_patch` program
//! is installed. It is approved as an `edit` and recorded as the shell call with
//! `metadata.intercepted`.

use std::path::Path;

use serde_json::json;

use super::ToolError;
use crate::host::Ctx;

/// The patch a shell script carries, and the directory its `cd` moves to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ShellPatch<'a> {
    pub dir: Option<String>,
    pub patch: &'a str,
}

/// The patch inside a heredoc wrapper (`<<'EOF'` … `EOF`, optionally after
/// `apply_patch`), for a patch sent with its shell wrapping still on.
pub(crate) fn unwrap_heredoc(text: &str) -> Option<&str> {
    let (before, body) = heredoc(text)?;
    matches!(before, "" | "apply_patch").then_some(body)
}

/// `script` as a patch, when the whole script is `[cd <dir> &&] apply_patch <heredoc>`.
pub(crate) fn shell_patch(script: &str) -> Option<ShellPatch<'_>> {
    let (before, patch) = heredoc(script.trim())?;
    let (dir, program) = match before.split_once("&&") {
        Some((cd, program)) => (Some(cd_target(cd)?), program.trim()),
        None => (None, before),
    };
    (program == "apply_patch").then_some(ShellPatch { dir, patch })
}

/// Run `found` as `apply_patch` relative to `workdir` and its `cd`, noting the
/// interception on the call's record.
pub(crate) async fn run(
    ctx: &Ctx<'_>,
    found: ShellPatch<'_>,
    workdir: &Path,
) -> Result<String, ToolError> {
    let base = match &found.dir {
        Some(dir) => workdir.join(dir),
        None => workdir.to_path_buf(),
    };
    ctx.host.note_call_metadata(
        &ctx.inv.session_id,
        &ctx.inv.call_id,
        json!({ "intercepted": "apply_patch" }),
    );
    super::patch::apply(ctx, found.patch, Some(&base)).await
}

/// Whether an `apply_patch` program is on the host's `PATH`; the script then runs as
/// written.
pub(crate) fn program_installed(path: &str) -> bool {
    const NAMES: &[&str] = if cfg!(windows) {
        &[
            "apply_patch.exe",
            "apply_patch.cmd",
            "apply_patch.bat",
            "apply_patch.com",
            "apply_patch.ps1",
        ]
    } else {
        &["apply_patch"]
    };
    std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .any(|dir| NAMES.iter().any(|name| executable(&dir.join(name))))
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    path.is_file()
}

/// Split a heredoc into the command before its opener and its body. The closing line
/// must be the last line. Shell heredocs (`<<EOF`, `<<'EOF'`, `<<-"EOF"`) and PowerShell
/// here-strings (`@'` … `'@`) both qualify.
fn heredoc(text: &str) -> Option<(&str, &str)> {
    let (first, rest) = text.split_once('\n')?;
    let (before, closer) = opener(first.trim_end())?;
    let (body, last) = rest.trim_end().rsplit_once('\n').unwrap_or(("", rest));
    (last.trim() == closer).then_some((before, body.trim_end_matches('\r')))
}

/// The command before a heredoc opener and the line that closes it.
fn opener(line: &str) -> Option<(&str, String)> {
    if let Some(at) = line.find("<<") {
        let word = unquote(line[at + 2..].trim_start_matches('-').trim())?;
        let valid = !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        return valid.then(|| (line[..at].trim_end(), word.to_owned()));
    }
    let quote = line.chars().last().filter(|q| matches!(q, '\'' | '"'))?;
    let before = line.strip_suffix(quote)?.strip_suffix('@')?;
    Some((before.trim_end(), format!("{quote}@")))
}

/// `cd <dir>` as its directory; a quoted directory may hold spaces. Anything the shell
/// would expand is refused, so the script then runs as written.
fn cd_target(cd: &str) -> Option<String> {
    let (command, dir) = cd.trim().split_once(char::is_whitespace)?;
    let dir = unquote(dir.trim())?;
    let plain = !dir.is_empty()
        && !dir.contains(['$', '`', '~', ';', '|', '&', '*', '?', '<', '>', '(', ')']);
    (command == "cd" && plain).then(|| dir.to_owned())
}

/// `text` without one pair of matching surrounding quotes; an unquoted word must not
/// contain whitespace.
fn unquote(text: &str) -> Option<&str> {
    match text.chars().next() {
        Some(quote @ ('\'' | '"')) => text.strip_prefix(quote)?.strip_suffix(quote),
        _ => (!text.contains(char::is_whitespace)).then_some(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "*** Begin Patch\n*** Delete File: a.txt\n*** End Patch";

    #[test]
    fn recognizes_heredoc_scripts_with_an_optional_cd() {
        let plain = format!("apply_patch <<'EOF'\n{PATCH}\nEOF\n");
        assert_eq!(
            shell_patch(&plain),
            Some(ShellPatch {
                dir: None,
                patch: PATCH
            })
        );
        let moved = format!("cd \"sub dir\" && apply_patch <<\"PATCH\"\n{PATCH}\nPATCH");
        assert_eq!(shell_patch(&moved).unwrap().dir.as_deref(), Some("sub dir"));
        let bare = format!(
            "apply_patch <<EOF\r\n{}\r\nEOF",
            PATCH.replace('\n', "\r\n")
        );
        assert_eq!(
            shell_patch(&bare).unwrap().patch,
            PATCH.replace('\n', "\r\n")
        );
        let powershell = format!("apply_patch @'\n{PATCH}\n'@");
        assert_eq!(shell_patch(&powershell).unwrap().patch, PATCH);
    }

    #[test]
    fn other_scripts_run_as_written() {
        for script in [
            format!("apply_patch <<'EOF'\n{PATCH}\nEOF\necho done"),
            format!("cat <<'EOF'\n{PATCH}\nEOF"),
            format!("cd $HOME && apply_patch <<'EOF'\n{PATCH}\nEOF"),
            format!("pushd src && apply_patch <<'EOF'\n{PATCH}\nEOF"),
            format!("apply_patch <<'EOF'\n{PATCH}\nEND"),
            "apply_patch patch.diff".to_string(),
        ] {
            assert_eq!(shell_patch(&script), None, "{script}");
        }
    }

    #[test]
    fn unwraps_a_heredoc_wrapped_patch() {
        let wrapped = format!("<<'EOF'\n{PATCH}\nEOF");
        assert_eq!(unwrap_heredoc(&wrapped), Some(PATCH));
        assert_eq!(unwrap_heredoc(PATCH), None);
    }

    #[test]
    fn finds_an_installed_program_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        let path = path.to_str().unwrap();
        assert!(!program_installed(path));
        let name = if cfg!(windows) {
            "apply_patch.exe"
        } else {
            "apply_patch"
        };
        let program = dir.path().join(name);
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(program_installed(path));
        assert!(!program_installed(""));
    }
}
