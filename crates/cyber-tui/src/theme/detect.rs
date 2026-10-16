//! Terminal color capability and background detection (`tui` → Color capability and
//! NO_COLOR, Theme tokens).
//!
//! Background detection never blocks a frame. The first frame uses `COLORFGBG` (or
//! dark). The OSC 11 reply arrives through crossterm's key stream as `Alt+]`, the
//! reply characters and `Alt+\` or `Ctrl+G`; [`Osc11Reply`] swallows those keys so they
//! never reach the composer and re-resolves the theme once the reply parses.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::color::Depth;

/// The query, terminated by ST.
pub const OSC11_QUERY: &str = "\u{1b}]11;?\u{1b}\\";

/// How long after the query a reply is still recognised, swallowed and applied.
/// Nothing waits for it, so a slow terminal costs no startup time.
pub const OSC11_LATE_WINDOW: Duration = Duration::from_secs(2);

/// The longest reply body accepted (`11;rgb:ffff/ffff/ffff` is 22 characters).
const MAX_REPLY: usize = 48;

/// Color depth from `NO_COLOR`, then `tui.color`, then `COLORTERM` and `TERM`.
pub fn depth(env: &dyn Fn(&str) -> Option<String>, setting: Option<&str>) -> Depth {
    if env("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return Depth::None;
    }
    match setting {
        Some("none") => return Depth::None,
        Some("truecolor") => return Depth::TrueColor,
        Some("256") => return Depth::Ansi256,
        Some("16") => return Depth::Ansi16,
        _ => {}
    }
    let colorterm = env("COLORTERM").unwrap_or_default();
    if matches!(colorterm.as_str(), "truecolor" | "24bit") || env("WT_SESSION").is_some() {
        Depth::TrueColor
    } else if env("TERM").is_some_and(|t| t.contains("256color")) {
        Depth::Ansi256
    } else {
        Depth::Ansi16
    }
}

/// `tui.theme_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    Auto,
    Dark,
    Light,
}

impl ThemeMode {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("dark") => Self::Dark,
            Some("light") => Self::Light,
            _ => Self::Auto,
        }
    }
}

/// `COLORFGBG` is `fg;bg` (or `fg;default;bg`): background 7 or 9–15 is light.
pub fn colorfgbg_is_light(value: &str) -> Option<bool> {
    let bg: u8 = value.rsplit(';').next()?.trim().parse().ok()?;
    Some(bg == 7 || (9..=15).contains(&bg))
}

/// The background color in an OSC 11 reply body (`11;rgb:RRRR/GGGG/BBBB`, 1–4 hex
/// digits per channel), as 8-bit channels.
pub fn parse_osc11(body: &str) -> Option<(u8, u8, u8)> {
    let rgb = body.strip_prefix("11;rgb:")?;
    let mut channels = rgb.split('/').map(|c| {
        if c.is_empty() || c.len() > 4 || !c.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(c, 16).ok()?;
        let max = (1u32 << (4 * c.len() as u32)) - 1;
        Some(((value * 255 + max / 2) / max) as u8)
    });
    let (r, g, b) = (channels.next()??, channels.next()??, channels.next()??);
    channels.next().is_none().then_some((r, g, b))
}

/// Whether a background color is light (luminance above the midpoint).
pub fn is_light(rgb: (u8, u8, u8)) -> bool {
    let color = ratatui::style::Color::Rgb(rgb.0, rgb.1, rgb.2);
    super::color::luminance(color).is_some_and(|l| l > 0.5)
}

/// What a key did to an outstanding OSC 11 query.
#[derive(Debug, Clone, PartialEq)]
pub enum Feed {
    /// Not part of a reply: handle it normally.
    Pass(KeyEvent),
    /// Part of a reply: drop it.
    Swallowed,
    /// The reply ended; `None` when it did not parse.
    Done(Option<(u8, u8, u8)>),
}

/// Collects one OSC 11 reply from the key stream.
#[derive(Debug, Clone)]
pub struct Osc11Reply {
    sent: Instant,
    body: Option<String>,
}

impl Osc11Reply {
    pub fn new(sent: Instant) -> Self {
        Self { sent, body: None }
    }

    /// Whether the reply is still expected at `now`.
    pub fn waiting(&self, now: Instant) -> bool {
        self.body.is_some() || now.duration_since(self.sent) < OSC11_LATE_WINDOW
    }

    pub fn feed(&mut self, key: KeyEvent, now: Instant) -> Feed {
        let Some(body) = self.body.as_mut() else {
            if key.code == KeyCode::Char(']')
                && key.modifiers == KeyModifiers::ALT
                && self.waiting(now)
            {
                self.body = Some(String::new());
                return Feed::Swallowed;
            }
            return Feed::Pass(key);
        };
        let end = matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('\\'), KeyModifiers::ALT) | (KeyCode::Char('g'), KeyModifiers::CONTROL)
        );
        match key.code {
            _ if end => Feed::Done(parse_osc11(body)),
            KeyCode::Char(c)
                if body.len() < MAX_REPLY
                    && key.modifiers.difference(KeyModifiers::SHIFT).is_empty() =>
            {
                body.push(c);
                Feed::Swallowed
            }
            _ => Feed::Done(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn depth_follows_no_color_then_the_setting_then_the_terminal() {
        let truecolor = env(&[("COLORTERM", "truecolor"), ("TERM", "xterm-256color")]);
        assert_eq!(depth(&truecolor, None), Depth::TrueColor);
        assert_eq!(depth(&truecolor, Some("16")), Depth::Ansi16);
        assert_eq!(depth(&truecolor, Some("none")), Depth::None);
        assert_eq!(
            depth(&env(&[("TERM", "xterm-256color")]), None),
            Depth::Ansi256
        );
        assert_eq!(
            depth(&env(&[("TERM", "xterm")]), Some("auto")),
            Depth::Ansi16
        );
        assert_eq!(depth(&env(&[("WT_SESSION", "1")]), None), Depth::TrueColor);
        let no_color = env(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")]);
        assert_eq!(depth(&no_color, Some("truecolor")), Depth::None);
        let empty = env(&[("NO_COLOR", ""), ("COLORTERM", "24bit")]);
        assert_eq!(depth(&empty, None), Depth::TrueColor);
    }

    #[test]
    fn colorfgbg_and_osc11_replies_decide_light_backgrounds() {
        assert_eq!(colorfgbg_is_light("0;15"), Some(true));
        assert_eq!(colorfgbg_is_light("15;default;0"), Some(false));
        assert_eq!(colorfgbg_is_light("garbage"), None);
        assert_eq!(parse_osc11("11;rgb:ffff/ffff/ffff"), Some((255, 255, 255)));
        assert_eq!(parse_osc11("11;rgb:1e/1e/2e"), Some((0x1e, 0x1e, 0x2e)));
        assert_eq!(parse_osc11("11;rgb:ffff/ffff"), None);
        assert_eq!(parse_osc11("11;rgb:fffff/0/0"), None);
        assert_eq!(parse_osc11("10;rgb:ffff/ffff/ffff"), None);
        assert!(is_light((255, 255, 255)));
        assert!(!is_light((0x1e, 0x1e, 0x2e)));
    }

    fn feed_text(reply: &mut Osc11Reply, text: &str, now: Instant) -> Vec<Feed> {
        text.chars()
            .map(|c| reply.feed(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), now))
            .collect()
    }

    #[test]
    fn a_reply_in_the_key_stream_is_swallowed_and_parsed() {
        let sent = Instant::now();
        let mut reply = Osc11Reply::new(sent);
        let start = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT);
        assert_eq!(reply.feed(start, sent), Feed::Swallowed);
        assert!(
            feed_text(&mut reply, "11;rgb:ffff/ffff/ffff", sent)
                .iter()
                .all(|f| *f == Feed::Swallowed)
        );
        let end = KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::ALT);
        assert_eq!(reply.feed(end, sent), Feed::Done(Some((255, 255, 255))));
    }

    #[test]
    fn ordinary_keys_pass_and_a_late_or_runaway_reply_is_bounded() {
        let sent = Instant::now();
        let mut reply = Osc11Reply::new(sent);
        let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(reply.feed(a, sent), Feed::Pass(a));
        let start = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT);
        let late = sent + OSC11_LATE_WINDOW;
        assert_eq!(reply.feed(start, late), Feed::Pass(start));
        assert_eq!(reply.feed(start, sent), Feed::Swallowed);
        let fed = feed_text(&mut reply, &"x".repeat(MAX_REPLY + 1), sent);
        assert_eq!(fed.last(), Some(&Feed::Done(None)));
    }
}
