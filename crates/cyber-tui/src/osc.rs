//! Terminal side channels for transcript content: OSC 8 hyperlinks over drawn link text,
//! and copying through a native clipboard tool or OSC 52 (`tui` → Markdown rendering,
//! Syntax-highlighted code blocks). The text and URLs come from model output, so every
//! byte written inside a sequence is checked first.

use std::io::{self, Write};

use ratatui::backend::IntoCrossterm;
use ratatui::buffer::Buffer;
use ratatui::crossterm::cursor::{MoveTo, RestorePosition, SavePosition};
use ratatui::crossterm::queue;
use ratatui::crossterm::style::{Print, ResetColor, SetAttribute, SetStyle};
use ratatui::style::Style;
use unicode_width::UnicodeWidthChar;

use crate::markdown::osc8_safe;

/// The largest copy sent through OSC 52; terminals drop longer ones.
pub const OSC52_MAX: usize = 100_000;

/// Link text drawn at `x`, `y` in the last frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenLink {
    pub x: u16,
    pub y: u16,
    pub text: String,
    pub url: String,
}

/// Whether the terminal shows OSC 8 hyperlinks. Multiplexers are left out: they pass
/// the sequence on only when configured to.
pub fn hyperlinks(env: &dyn Fn(&str) -> Option<String>) -> bool {
    let term = env("TERM").unwrap_or_default();
    if env("TMUX").is_some() || term.starts_with("screen") || term.starts_with("tmux") {
        return false;
    }
    let program = env("TERM_PROGRAM").unwrap_or_default();
    let known_program = matches!(
        program.as_str(),
        "iTerm.app" | "WezTerm" | "vscode" | "ghostty" | "Hyper" | "rio"
    );
    let vte = env("VTE_VERSION")
        .and_then(|v| v.parse::<u32>().ok())
        .is_some_and(|v| v >= 5000);
    let known_term = ["kitty", "foot", "alacritty", "ghostty", "wezterm"]
        .iter()
        .any(|name| term.contains(name));
    known_program
        || vte
        || known_term
        || env("WT_SESSION").is_some()
        || env("KITTY_WINDOW_ID").is_some()
}

/// Redraw each link whose text is still on screen inside an OSC 8 sequence, in the
/// style its cells have. A link an overlay covers, or with an unsafe URL, is skipped.
pub fn write_hyperlinks(
    out: &mut impl Write,
    buf: &Buffer,
    links: &[ScreenLink],
) -> io::Result<()> {
    let shown: Vec<&ScreenLink> = links
        .iter()
        .filter(|l| osc8_safe(&l.url) && on_screen(buf, l))
        .collect();
    if shown.is_empty() {
        return Ok(());
    }
    queue!(out, SavePosition)?;
    for link in shown {
        let cell = &buf[(link.x, link.y)];
        let style = Style::default()
            .fg(cell.fg)
            .bg(cell.bg)
            .add_modifier(cell.modifier);
        queue!(
            out,
            MoveTo(link.x, link.y),
            SetStyle(style.into_crossterm()),
            Print(format!("\x1b]8;;{}\x1b\\", link.url)),
            Print(&link.text),
            Print("\x1b]8;;\x1b\\"),
            SetAttribute(ratatui::crossterm::style::Attribute::Reset),
            ResetColor
        )?;
    }
    queue!(out, RestorePosition)?;
    out.flush()
}

/// The buffer holds exactly `link.text` (printable characters only) from its position.
fn on_screen(buf: &Buffer, link: &ScreenLink) -> bool {
    let mut x = link.x;
    for ch in link.text.chars() {
        if ch.is_control() || !buf.area.contains((x, link.y).into()) {
            return false;
        }
        if buf[(x, link.y)].symbol() != ch.encode_utf8(&mut [0; 4]) {
            return false;
        }
        x = x.saturating_add(ch.width().unwrap_or(0) as u16);
    }
    !link.text.is_empty()
}

/// How a copy reached the clipboard, for the toast.
#[derive(Debug, Clone, PartialEq)]
pub enum Copied {
    Native(&'static str),
    Osc52,
}

/// The native clipboard tools to try, in order. None over SSH, where they would fill
/// the remote machine's clipboard instead of the user's.
pub fn native_tools(
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<(&'static str, &'static [&'static str])> {
    if env("SSH_CONNECTION").is_some() || env("SSH_TTY").is_some() {
        return Vec::new();
    }
    if cfg!(target_os = "macos") {
        return vec![("pbcopy", &[])];
    }
    if cfg!(windows) {
        return Vec::new();
    }
    let mut tools: Vec<(&'static str, &'static [&'static str])> = Vec::new();
    if env("WAYLAND_DISPLAY").is_some() {
        tools.push(("wl-copy", &[]));
    }
    if env("DISPLAY").is_some() {
        tools.push(("xclip", &["-selection", "clipboard"]));
        tools.push(("xsel", &["--clipboard", "--input"]));
    }
    tools
}

/// `text` as the screen drew it: control characters other than newline and tab, and the
/// bidi embedding, override and isolate controls, are removed. Copied text comes from
/// model output and untrusted repositories, so an ESC or a bracketed-paste end marker
/// on the clipboard would break out of the shell's paste and run what follows.
pub fn clipboard_safe(text: &str) -> String {
    text.chars()
        .filter(|&c| {
            let control = c.is_control() && !matches!(c, '\n' | '\t');
            let bidi = matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
            !control && !bidi
        })
        .collect()
}

/// Copy `text`, cleaned by [`clipboard_safe`]: the first native tool that accepts it,
/// else an OSC 52 sequence on `out`.
pub fn copy(
    text: &str,
    env: &dyn Fn(&str) -> Option<String>,
    out: &mut impl Write,
) -> Result<Copied, String> {
    let text = clipboard_safe(text);
    let text = text.as_str();
    for (tool, args) in native_tools(env) {
        if pipe(tool, args, text).is_ok() {
            return Ok(Copied::Native(tool));
        }
    }
    let sequence = osc52(text).ok_or("too large to copy through the terminal")?;
    out.write_all(sequence.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())?;
    Ok(Copied::Osc52)
}

fn pipe(tool: &str, args: &[&str], text: &str) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(tool)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{tool} exited with {status}")))
    }
}

/// The OSC 52 sequence that sets the clipboard to `text`; base64 keeps every byte of
/// the payload printable. `None` when it is over [`OSC52_MAX`].
pub fn osc52(text: &str) -> Option<String> {
    let encoded = base64(text.as_bytes());
    (encoded.len() <= OSC52_MAX).then(|| format!("\x1b]52;c;{encoded}\x07"))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
    }

    #[test]
    fn osc52_payload_is_printable_and_bounded() {
        let sequence = osc52("print(\"\x1b]0;pwned\x07\")\n").unwrap();
        let payload = sequence
            .strip_prefix("\x1b]52;c;")
            .and_then(|s| s.strip_suffix('\x07'))
            .unwrap();
        assert!(
            payload
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        );
        assert!(osc52(&"x".repeat(OSC52_MAX)).is_none());
    }

    #[test]
    fn copy_falls_back_to_osc52_without_a_native_clipboard() {
        let ssh = env(&[("SSH_CONNECTION", "1 2 3 4"), ("DISPLAY", ":0")]);
        assert!(native_tools(&ssh).is_empty());
        let mut out = Vec::new();
        assert_eq!(copy("fn main() {}", &ssh, &mut out), Ok(Copied::Osc52));
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("\x1b]52;c;{}\x07", base64(b"fn main() {}"))
        );
    }

    #[test]
    fn copy_strips_controls_that_could_escape_a_bracketed_paste() {
        assert_eq!(
            clipboard_safe("echo ok\x1b[201~\r\nrm -rf ~\u{202E}\u{2066}\x07\tend\n"),
            "echo ok[201~\nrm -rf ~\tend\n"
        );
        let ssh = env(&[("SSH_CONNECTION", "1 2 3 4")]);
        let mut out = Vec::new();
        assert_eq!(copy("a\x1b[201~\rb", &ssh, &mut out), Ok(Copied::Osc52));
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("\x1b]52;c;{}\x07", base64(b"a[201~b"))
        );
    }

    #[test]
    fn hyperlink_support_is_detected_conservatively() {
        assert!(hyperlinks(&env(&[("TERM_PROGRAM", "iTerm.app")])));
        assert!(hyperlinks(&env(&[("WT_SESSION", "x")])));
        assert!(hyperlinks(&env(&[("VTE_VERSION", "6003")])));
        assert!(!hyperlinks(&env(&[("VTE_VERSION", "4000")])));
        assert!(!hyperlinks(&env(&[("TERM", "xterm-256color")])));
        assert!(!hyperlinks(&env(&[
            ("TERM_PROGRAM", "iTerm.app"),
            ("TMUX", "/tmp/t")
        ])));
    }

    fn buffer_with(text: &str) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 2));
        buf.set_string(2, 1, text, Style::default());
        buf
    }

    #[test]
    fn hyperlinks_are_written_only_over_matching_text_with_safe_urls() {
        let buf = buffer_with("docs here");
        let link = |text: &str, url: &str| ScreenLink {
            x: 2,
            y: 1,
            text: text.into(),
            url: url.into(),
        };
        let mut out = Vec::new();
        write_hyperlinks(&mut out, &buf, &[link("docs", "https://example.com/a")]).unwrap();
        let written = String::from_utf8(out).unwrap();
        assert!(written.contains("\x1b]8;;https://example.com/a\x1b\\docs\x1b]8;;\x1b\\"));
        for bad in [
            link("docs", "https://example.com/\x1b]52;c;eA==\x07"),
            link("docs", "javascript:alert(1)"),
            link("docs", "https://example.com/a b"),
            link("covered", "https://example.com/a"),
        ] {
            let mut out = Vec::new();
            write_hyperlinks(&mut out, &buf, &[bad]).unwrap();
            assert!(out.is_empty());
        }
    }
}
