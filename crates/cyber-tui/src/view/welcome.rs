//! The welcome screen (`tui` → Welcome screen): the logo, the version line, a panel
//! built from the sidebar's data, and tips from the binding table. It is drawn in the
//! transcript area of a Session with no messages and is never part of the transcript.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::layout::truncate_end;
use crate::app::App;
use crate::theme::Depth;
use crate::theme::color::{blend, reduce};

const WIDE: &str = include_str!("../../assets/logo-wide.txt");
const COMPACT: &str = include_str!("../../assets/logo-compact.txt");
/// The wide logo needs this many transcript columns; the compact one, [`COMPACT_FROM`].
const WIDE_FROM: u16 = 82;
const COMPACT_FROM: u16 = 45;
const TAGLINE: &str = "local-first coding agent";
const PANEL_MAX: usize = 64;
const LABEL: usize = 7;
/// Binding actions the tips list, in order.
const TIPS: &[&str] = &[
    "command_palette",
    "sidebar_toggle",
    "model_list",
    "session_list",
    "help",
];

/// `tui.welcome`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Welcome {
    #[default]
    Full,
    /// The panel and tips without the logo.
    Compact,
    Off,
}

impl Welcome {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value? {
            "full" => Some(Self::Full),
            "compact" => Some(Self::Compact),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// Shown instead of the transcript: no message exists yet and it is not turned off.
pub fn shown(app: &App) -> bool {
    app.welcome != Welcome::Off && app.items.is_empty() && app.streaming.is_empty()
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if app.screen_reader {
        let mut lines = vec![Line::from(version_text(app, "Cyber Code"))];
        lines.extend(
            panel_rows(app)
                .into_iter()
                .map(|(label, value)| Line::from(format!("{label}: {value}"))),
        );
        f.render_widget(Paragraph::new(lines), area);
        return;
    }
    let lines = layout(app, area);
    let top = area.height.saturating_sub(lines.len() as u16) / 2;
    let rect = Rect {
        y: area.y + top,
        height: area.height - top,
        ..area
    };
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), rect);
}

/// The blocks that fit `area`: the logo goes first when it and the panel do not fit,
/// then the tips.
fn layout(app: &App, area: Rect) -> Vec<Line<'static>> {
    let logo = if app.welcome == Welcome::Full {
        logo(app, area.width)
    } else {
        Vec::new()
    };
    let version = vec![Line::from(Span::styled(
        version_text(app, ""),
        Style::default().fg(app.theme.text_muted),
    ))];
    let panel = panel(app, area.width as usize);
    let tips = tips(app, area.width as usize);
    let height = area.height as usize;
    let fits = |blocks: &[&Vec<Line<'static>>]| {
        blocks.iter().map(|b| b.len()).sum::<usize>() + blocks.len() - 1 <= height
    };
    // The logo goes when it and the panel do not fit; tips only fill the room left.
    let blocks: Vec<&Vec<Line<'static>>> = [
        vec![&logo, &version, &panel, &tips],
        vec![&logo, &version, &panel],
        vec![&version, &panel, &tips],
        vec![&version, &panel],
    ]
    .into_iter()
    .map(|blocks| {
        blocks
            .into_iter()
            .filter(|b| !b.is_empty())
            .collect::<Vec<_>>()
    })
    .find(|blocks| fits(blocks))
    .unwrap_or_else(|| vec![&panel]);
    let mut out = Vec::new();
    for (i, block) in blocks.into_iter().enumerate() {
        if i > 0 {
            out.push(Line::default());
        }
        out.extend(block.iter().cloned());
    }
    out
}

fn version_text(app: &App, prefix: &str) -> String {
    let version = app.sidebar.version.as_deref().unwrap_or_default();
    let head = [prefix, version]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if prefix.is_empty() {
        [head.as_str(), TAGLINE]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    } else {
        head
    }
}

/// The logo for the transcript width, colored with the `accent` → `secondary` gradient.
fn logo(app: &App, width: u16) -> Vec<Line<'static>> {
    let art = if width >= WIDE_FROM {
        WIDE
    } else if width >= COMPACT_FROM {
        COMPACT
    } else {
        return vec![Line::from(Span::styled(
            "Cyber Code",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))];
    };
    art_lines(app, &art.lines().collect::<Vec<_>>())
}

/// The first word of the compact logo (`CYBER`) in the theme's gradient, for the theme
/// switcher's preview.
pub(crate) fn preview_logo(app: &App) -> Vec<Line<'static>> {
    art_lines(app, &COMPACT.lines().take(6).collect::<Vec<_>>())
}

fn art_lines(app: &App, rows: &[&str]) -> Vec<Line<'static>> {
    let columns = rows.iter().map(|r| r.width()).max().unwrap_or(0);
    let palette = Gradient::of(app);
    rows.iter()
        .map(|row| {
            let mut padded: Vec<char> = row.chars().collect();
            padded.resize(columns, ' ');
            Line::from(
                padded
                    .iter()
                    .enumerate()
                    .map(|(x, &c)| Span::styled(c.to_string(), palette.style(c, x, columns)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// The logo's colors at the terminal's depth.
enum Gradient {
    /// `NO_COLOR` or monochrome: glyphs only.
    None,
    /// 16 colors: `accent` for blocks, dimmed for shadows.
    Accent(Color),
    /// RGB endpoints and the background shadows blend toward.
    Rgb {
        from: Color,
        to: Color,
        background: Color,
        depth: Depth,
    },
}

impl Gradient {
    fn of(app: &App) -> Self {
        let t = &app.theme;
        match app.appearance.depth {
            _ if t.monochrome => Self::None,
            Depth::None => Self::None,
            Depth::Ansi16 => Self::Accent(t.accent),
            depth => {
                let full = app.themes.get(&t.name).resolve(t.light, Depth::TrueColor);
                Self::Rgb {
                    from: full.accent,
                    to: full.secondary,
                    background: full.background,
                    depth,
                }
            }
        }
    }

    fn style(&self, glyph: char, x: usize, columns: usize) -> Style {
        let shadow = glyph != '█';
        match self {
            _ if glyph == ' ' => Style::default(),
            Self::None => Style::default(),
            Self::Accent(color) if shadow => {
                Style::default().fg(*color).add_modifier(Modifier::DIM)
            }
            Self::Accent(color) => Style::default().fg(*color),
            Self::Rgb {
                from,
                to,
                background,
                depth,
            } => {
                let at = x as f32 / columns.saturating_sub(1).max(1) as f32;
                let mut color = blend(*from, *to, at);
                if shadow {
                    color = blend(color, *background, 0.55);
                }
                Style::default().fg(reduce(color, *depth))
            }
        }
    }
}

/// `(label, value)` rows from the Session, the catalog and the sidebar's sources.
fn panel_rows(app: &App) -> Vec<(&'static str, String)> {
    let s = &app.session;
    let (provider, variant) = super::composer::provider_and_variant(&s.model);
    let mut model = format!("{} {provider}", app.catalog.model_name(&s.model));
    if let Some(variant) = variant {
        model.push_str(&format!(" · {variant}"));
    }
    let location: String = super::sidebar::location_spans(app)
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let mut rows = vec![
        ("Agent", crate::turns::title_case(&s.agent)),
        ("Model", model.trim().to_owned()),
        ("Mode", app.mode_label()),
        ("Path", location),
    ];
    if let Some(summary) = mcp_summary(app) {
        rows.push(("MCP", summary));
    }
    if let Some(servers) = app.sidebar.lsp.ready() {
        let active: Vec<&str> = servers
            .iter()
            .filter(|s| s.status == "connected")
            .map(|s| s.id.as_str())
            .collect();
        let text = if active.is_empty() {
            "LSPs will activate as files are read".to_owned()
        } else {
            active.join(", ")
        };
        rows.push(("LSP", text));
    }
    rows
}

/// `● 2 connected · ● 1 failed`, or `None` without MCP data or servers.
fn mcp_summary(app: &App) -> Option<String> {
    let servers = app.sidebar.mcp.ready()?;
    if servers.is_empty() {
        return None;
    }
    let count = |status: &str| servers.iter().filter(|s| s.status == status).count();
    let mut parts = vec![format!("● {} connected", count("connected"))];
    if count("failed") > 0 {
        parts.push(format!("● {} failed", count("failed")));
    }
    Some(parts.join(" · "))
}

/// The panel on `surface`, every row padded to one width so centering keeps it a box.
fn panel(app: &App, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let inner = width.saturating_sub(6).min(PANEL_MAX);
    let surface = Style::default().fg(t.text).bg(t.surface);
    let blank = || Line::from(Span::styled(" ".repeat(inner + 4), surface));
    let mut lines = vec![blank()];
    for (label, value) in panel_rows(app) {
        let value = truncate_end(&value, inner.saturating_sub(LABEL));
        let fill = inner.saturating_sub(LABEL + value.width());
        let value_style = if label == "MCP" && value.contains("failed") {
            surface.fg(t.error)
        } else {
            surface
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {label:<LABEL$}"), surface.fg(t.text_muted)),
            Span::styled(value, value_style),
            Span::styled(" ".repeat(fill + 2), surface),
        ]));
    }
    lines.push(blank());
    lines
}

/// `Tips` and one row per enabled binding, with its current key.
fn tips(app: &App, width: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let inner = width.saturating_sub(6).min(PANEL_MAX);
    let hints: Vec<(String, &str)> = TIPS
        .iter()
        .filter_map(|action| app.keymap.hint("global", action))
        .collect();
    if hints.is_empty() {
        return Vec::new();
    }
    let key_width = hints.iter().map(|(k, _)| k.width()).max().unwrap_or(0) + 2;
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<inner$}", "Tips"),
        Style::default().fg(t.text).add_modifier(Modifier::BOLD),
    ))];
    for (key, description) in hints {
        let text = truncate_end(description, inner.saturating_sub(key_width));
        let fill = inner.saturating_sub(key_width + text.width());
        lines.push(Line::from(vec![
            Span::styled(format!("{key:<key_width$}"), Style::default().fg(t.accent)),
            Span::styled(text, Style::default().fg(t.text_muted)),
            Span::raw(" ".repeat(fill)),
        ]));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logos_match_their_documented_sizes() {
        let wide: Vec<&str> = WIDE.lines().collect();
        assert_eq!(wide.len(), 6);
        assert_eq!(wide.iter().map(|r| r.width()).max(), Some(78));
        let compact: Vec<&str> = COMPACT.lines().collect();
        assert_eq!(compact.len(), 12);
        assert_eq!(compact.iter().map(|r| r.width()).max(), Some(41));
        assert_eq!(Welcome::parse(Some("compact")), Some(Welcome::Compact));
        assert_eq!(Welcome::parse(Some("big")), None);
    }
}
