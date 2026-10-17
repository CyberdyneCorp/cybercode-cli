//! The responsive layout (`tui` → Responsive layout): one [`LayoutPlan`] per frame from
//! the terminal size and preferences, so every region is computed in one place and no
//! region draws outside its area.

use ratatui::layout::{Constraint, Layout, Rect};
use unicode_width::UnicodeWidthChar;

/// The sidebar docks from this width.
pub const DOCK_FROM: u16 = 120;
/// Below this width the status footer shows only context percent and cost.
pub const COMPACT_BELOW: u16 = 60;
/// The smallest screen drawn; anything smaller shows only a message.
pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarMode {
    Docked,
    /// Over the right side of the transcript.
    Drawer,
    Hidden,
}

/// What the plan depends on besides the size.
#[derive(Debug, Clone, Copy, Default)]
pub struct Inputs {
    /// The sidebar would dock (preference and toggle) at this width.
    pub docked: bool,
    pub drawer: bool,
    /// Screen-reader mode draws no sidebar.
    pub screen_reader: bool,
    pub queued: u16,
    /// Rows of the progress line above the composer.
    pub progress: u16,
    pub composer: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutPlan {
    pub too_small: bool,
    /// Zero height when there is no header row.
    pub header: Rect,
    pub transcript: Rect,
    pub queued: Rect,
    pub progress: Rect,
    pub composer: Rect,
    pub footer: Rect,
    pub sidebar: Option<Rect>,
    pub mode: SidebarMode,
    pub compact_footer: bool,
}

/// 30% of the terminal, clamped to 32..48 columns.
pub fn sidebar_width(total: u16) -> u16 {
    (total.saturating_mul(3) / 10).clamp(32, 48)
}

pub fn plan(area: Rect, inputs: &Inputs) -> LayoutPlan {
    let empty = Rect { height: 0, ..area };
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return LayoutPlan {
            too_small: true,
            header: empty,
            transcript: empty,
            queued: empty,
            progress: empty,
            composer: empty,
            footer: empty,
            sidebar: None,
            mode: SidebarMode::Hidden,
            compact_footer: true,
        };
    }
    let mode = match (inputs.screen_reader, area.width >= DOCK_FROM) {
        (true, _) => SidebarMode::Hidden,
        (false, true) if inputs.docked => SidebarMode::Docked,
        (false, false) if inputs.drawer => SidebarMode::Drawer,
        _ => SidebarMode::Hidden,
    };
    let (main, docked) = if mode == SidebarMode::Docked {
        let [main, side] = Layout::horizontal([
            Constraint::Min(1),
            Constraint::Length(sidebar_width(area.width)),
        ])
        .areas(area);
        (main, Some(side))
    } else {
        (area, None)
    };
    let header_height = u16::from(mode != SidebarMode::Docked);
    let [header, transcript, queued, progress, composer, footer] = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(3),
        Constraint::Length(inputs.queued.min(4)),
        Constraint::Length(inputs.progress),
        Constraint::Length(inputs.composer),
        Constraint::Length(1),
    ])
    .areas(main);
    let sidebar = match mode {
        SidebarMode::Docked => docked,
        SidebarMode::Drawer => {
            let width = sidebar_width(area.width).min(transcript.width);
            Some(Rect {
                x: transcript.right() - width,
                width,
                ..transcript
            })
        }
        SidebarMode::Hidden => None,
    };
    LayoutPlan {
        too_small: false,
        header,
        transcript,
        queued,
        progress,
        composer,
        footer,
        sidebar,
        mode,
        compact_footer: area.width < COMPACT_BELOW,
    }
}

/// `text` cut to `width` columns, ending with `…` when it was cut.
pub fn truncate_end(text: &str, width: usize) -> String {
    if unicode_width::UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// A path shortened in the middle to fit `width` columns: whole components are kept
/// from both ends around `…` (`~/work/…/src/main.rs`), and a path whose ends do not
/// fit falls back to cutting characters in the middle.
pub fn shorten_path(path: &str, width: usize) -> String {
    let fits = |s: &str| unicode_width::UnicodeWidthStr::width(s) <= width;
    if fits(path) {
        return path.to_owned();
    }
    let separator = if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    };
    let parts: Vec<&str> = path.split(separator).collect();
    if parts.len() > 2 {
        for head in (1..parts.len() - 1).rev() {
            for tail in (1..parts.len() - head).rev() {
                let candidate = format!(
                    "{}{separator}…{separator}{}",
                    parts[..head].join(&separator.to_string()),
                    parts[parts.len() - tail..].join(&separator.to_string())
                );
                if fits(&candidate) {
                    return candidate;
                }
            }
        }
    }
    shorten_middle(path, width)
}

/// `/Users/leo/…/app` to fit `width` columns, cutting characters in the middle.
pub fn shorten_middle(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if unicode_width::UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    if width < 2 {
        return "…".chars().take(width).collect();
    }
    let keep = width - 1;
    let head = keep / 2;
    let tail = (keep - head).min(chars.len());
    let mut out: String = chars[..head.min(chars.len())].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(width: u16, height: u16, inputs: Inputs) -> LayoutPlan {
        plan(Rect::new(0, 0, width, height), &inputs)
    }

    fn inputs(docked: bool, drawer: bool) -> Inputs {
        Inputs {
            docked,
            drawer,
            composer: 3,
            ..Default::default()
        }
    }

    #[test]
    fn breakpoints_pick_docked_drawer_compact_and_too_small() {
        let wide = at(160, 48, inputs(true, false));
        assert_eq!(wide.mode, SidebarMode::Docked);
        assert_eq!(wide.sidebar.unwrap().width, 48);
        assert_eq!(wide.header.height, 0);
        assert_eq!(wide.transcript.width, 112);
        assert_eq!(at(120, 40, inputs(true, false)).sidebar.unwrap().width, 36);
        let medium = at(100, 30, inputs(true, false));
        assert_eq!(
            (medium.mode, medium.header.height),
            (SidebarMode::Hidden, 1)
        );
        let drawer = at(100, 30, inputs(true, true));
        assert_eq!(drawer.mode, SidebarMode::Drawer);
        let side = drawer.sidebar.unwrap();
        assert_eq!((side.width, side.right()), (32, 100));
        assert_eq!(side.y, drawer.transcript.y);
        assert!(!drawer.compact_footer && at(50, 20, inputs(false, false)).compact_footer);
        assert!(at(39, 20, inputs(false, false)).too_small);
        assert!(at(80, 9, inputs(false, false)).too_small);
        let reader = Inputs {
            screen_reader: true,
            ..inputs(true, true)
        };
        assert_eq!(at(160, 48, reader).mode, SidebarMode::Hidden);
    }

    #[test]
    fn paths_shorten_in_the_middle_by_component() {
        assert_eq!(
            shorten_path("~/work/cybercode/crates/app/src/main.rs", 22),
            "~/work/…/src/main.rs"
        );
        assert_eq!(shorten_path("~/a/b", 10), "~/a/b");
        assert_eq!(shorten_path("averyveryverylongname", 9), "aver…name");
        assert_eq!(shorten_middle("/Users/leo/work/app", 11), "/User…k/app");
        assert_eq!(truncate_end("spawn ENOENT", 8), "spawn E…");
        assert_eq!(truncate_end("ok", 8), "ok");
    }
}
