//! What the model types into a shell PTY: each submitted line is a shell command, which
//! `pty_write` authorizes as `bash` would before the shell sees it.

/// Programs whose typed lines are shell commands, also behind a wrapper (`env bash`,
/// `sudo -s`, `ssh host`).
const SHELLS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "mksh",
    "tcsh",
    "csh",
    "nu",
    "elvish",
    "xonsh",
    "busybox",
    "pwsh",
    "powershell",
    "cmd",
    "su",
    "sudo",
    "doas",
    "ssh",
    "script",
    "tmux",
    "screen",
];

/// Whether lines typed into a PTY running `command` (the shell path when no command was
/// given, which may be a Windows path) are shell commands. Shell syntax may end in a
/// shell, so it counts as one.
pub(super) fn is_shell(command: &str) -> bool {
    let Some(argv) = crate::bash_analysis::literal_argv(command) else {
        return true;
    };
    let program = command.split_whitespace().next().unwrap_or_default();
    std::iter::once(program)
        .chain(argv.iter().map(String::as_str))
        .any(names_a_shell)
}

fn names_a_shell(arg: &str) -> bool {
    let name = arg.rsplit(['/', '\\']).next().unwrap_or(arg);
    let name = name.to_ascii_lowercase();
    SHELLS.contains(&name.strip_suffix(".exe").unwrap_or(&name))
}

/// The commands `bytes` submits after the `pending` input, and the new pending input.
/// A shell reads a line ending in an unescaped backslash, or one that leaves a quote,
/// heredoc or compound command open, together with the lines after it, so a submitted
/// command is the joined text once it parses (backslash-newline pairs removed, as the
/// shell removes them). Input may end inside a backslash continuation, but not with an
/// open construct the shell could also read as complete: that is refused. Backspace,
/// ctrl-c and ctrl-u edit the input as a shell's line editor does; any other control key
/// (tab completion, history, cursor movement) could run text no check saw, so it is
/// refused.
pub(super) fn submit(pending: &[u8], bytes: &[u8]) -> Result<(Vec<String>, Vec<u8>), String> {
    let mut input = Input::resume(pending);
    for &byte in bytes {
        match byte {
            b'\r' | b'\n' => input.end_line(),
            0x7f | 0x08 => input.backspace(),
            0x03 => input = Input::default(),
            0x15 => input.raw.truncate(input.start),
            0x04 => {}
            byte if byte < 0x20 => {
                return Err(format!(
                    "A shell PTY accepts typed lines only: control key 0x{byte:02x} is refused. Type each command and end it with \\r."
                ));
            }
            byte => input.raw.push(byte),
        }
    }
    if input.open {
        return Err(
            "This input leaves a quote, heredoc or compound command open: send the whole command, through its closing line, in one pty_write.".into(),
        );
    }
    Ok((input.submitted, input.raw))
}

/// Typed input the shell has not run yet.
#[derive(Default)]
struct Input {
    /// Every byte since the last submitted command, as typed.
    raw: Vec<u8>,
    /// Where the line being typed starts in `raw`.
    start: usize,
    /// The finished lines of `raw` as the shell reads them: a continued line joined to
    /// the next, any other line ended by a newline.
    head: String,
    /// A finished line is open without a backslash continuation: the shell may also read
    /// it as complete, so the input must not end here.
    open: bool,
    submitted: Vec<String>,
}

impl Input {
    /// Pending input holds backslash continuations only (an open construct is refused).
    fn resume(pending: &[u8]) -> Self {
        let start = pending
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |at| at + 1);
        Self {
            raw: pending.to_vec(),
            start,
            head: String::from_utf8_lossy(&pending[..start]).replace("\\\n", ""),
            ..Self::default()
        }
    }

    /// Submit the joined input at a newline, or keep it pending when the shell reads on.
    fn end_line(&mut self) {
        let text = format!(
            "{}{}",
            self.head,
            String::from_utf8_lossy(&self.raw[self.start..])
        );
        if crate::bash_analysis::continues_line(&text) {
            self.head = text;
            self.head.pop();
        } else if !crate::bash_analysis::parses(&text) {
            self.head = text + "\n";
            self.open = true;
        } else {
            let command = text.trim();
            if !command.is_empty() {
                self.submitted.push(command.to_string());
            }
            *self = Self {
                submitted: std::mem::take(&mut self.submitted),
                ..Self::default()
            };
            return;
        }
        self.raw.push(b'\n');
        self.start = self.raw.len();
    }

    /// Remove the last character, with its UTF-8 continuation bytes; a finished line is
    /// out of reach, as in the shell's line editor.
    fn backspace(&mut self) {
        while self.raw.len() > self.start {
            let Some(byte) = self.raw.pop() else { break };
            if byte & 0xc0 != 0x80 {
                break;
            }
        }
    }
}

/// Whether a submitted command defines an alias: an interactive shell expands it in a
/// later line, where its value would run unchecked.
pub(super) fn defines_alias(analysis: &crate::bash_analysis::Analysis) -> bool {
    analysis.commands.iter().any(|command| {
        command
            .text
            .split_whitespace()
            .find(|word| !matches!(*word, "builtin" | "command" | "\\"))
            .is_some_and(|word| word.trim_start_matches('\\') == "alias")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_and_shell_syntax_are_shells() {
        for command in [
            "/bin/zsh",
            "bash -i",
            "env bash",
            "sudo -s",
            "C:\\Git\\bin\\bash.exe",
            "a && b",
        ] {
            assert!(is_shell(command), "{command}");
        }
        for command in ["python3", "node", "vim notes.txt", "less app.log"] {
            assert!(!is_shell(command), "{command}");
        }
    }

    #[test]
    fn lines_are_split_and_edited() {
        let (lines, rest) = submit(b"git pu", b"sh --force\r\nls").unwrap();
        assert_eq!(
            (lines, rest),
            (vec!["git push --force".to_string()], b"ls".to_vec())
        );
        let (lines, rest) = submit(b"", b"rm -rf x\x7f.\r").unwrap();
        assert_eq!((lines, rest), (vec!["rm -rf .".to_string()], Vec::new()));
        let (lines, _) = submit(b"git push", b"\x03ls\r").unwrap();
        assert_eq!(lines, ["ls"]);
        assert!(submit(b"", b"\x1b[A\r").is_err(), "history");
        assert!(submit(b"git pu", b"\t").is_err(), "completion");
    }

    #[test]
    fn continued_and_open_lines_are_submitted_joined() {
        let (lines, rest) = submit(b"", b"rm -rf \\\r.\r").unwrap();
        assert_eq!((lines, rest), (vec!["rm -rf .".to_string()], Vec::new()));
        let (lines, rest) = submit(b"", b"git \\\r").unwrap();
        assert!(lines.is_empty());
        let (lines, _) = submit(&rest, b"\x7f\x7fpush\r").unwrap();
        assert_eq!(
            lines,
            ["git push"],
            "a continued line is out of backspace's reach"
        );
        let (lines, _) = submit(b"", b"echo \"a\rb\"\rls\r").unwrap();
        assert_eq!(lines, ["echo \"a\nb\"", "ls"]);
        let (lines, _) = submit(b"", b"if true; then\r  rm -rf x\rfi\r").unwrap();
        assert_eq!(lines, ["if true; then\n  rm -rf x\nfi"]);
        assert!(
            submit(b"", b"echo 'open\r").is_err(),
            "an open quote ends the write"
        );
        assert!(
            submit(b"", b"for f in *; do\r").is_err(),
            "an open loop ends the write"
        );
        // A backslash in a comment does not continue the line.
        let (lines, _) = submit(b"", b"echo hi # note \\\rrm -rf x\r").unwrap();
        assert_eq!(lines, ["echo hi # note \\", "rm -rf x"]);
        let (lines, _) = submit(b"", b"echo a \\\\\rls\r").unwrap();
        assert_eq!(
            lines,
            ["echo a \\\\", "ls"],
            "an escaped backslash ends the line"
        );
    }

    #[test]
    fn alias_definitions_are_found() {
        let at = std::path::Path::new("/");
        for command in [
            "alias x='rm -rf .'",
            "builtin alias x=y",
            "ls && \\alias y=z",
        ] {
            let analysis = crate::bash_analysis::analyze(command, at);
            assert!(defines_alias(&analysis), "{command}");
        }
        let analysis = crate::bash_analysis::analyze("echo alias", at);
        assert!(!defines_alias(&analysis));
    }
}
