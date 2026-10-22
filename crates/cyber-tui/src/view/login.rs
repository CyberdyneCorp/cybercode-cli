//! The `/login` dialog: the authorization URL, whether the server waits for the browser,
//! and the pasted redirect URL.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::overlays::{centered, frame};
use crate::app::App;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let dialog = &app.login;
    let t = &app.theme;
    let rect = centered(area, 84, 16);
    let width = rect.width.saturating_sub(4) as usize;
    let muted = Style::default().fg(t.text_muted);
    let mut lines = if dialog.url.is_empty() {
        vec![Line::from(Span::styled("Starting the sign-in…", muted))]
    } else {
        vec![
            Line::from("Open this URL in a browser to sign in to ChatGPT:"),
            Line::from(Span::styled(
                dialog.url.clone(),
                Style::default().fg(t.accent),
            )),
            Line::default(),
            Line::from(Span::styled(waiting(dialog.callback), muted)),
        ]
    };
    let input = &dialog.input;
    let skip = input
        .chars()
        .count()
        .saturating_sub(width.saturating_sub(16));
    lines.push(Line::from(vec![
        Span::styled("Redirect URL: ", muted),
        Span::styled(
            input.chars().skip(skip).collect::<String>(),
            Style::default().fg(t.text),
        ),
        Span::styled("▏", Style::default().fg(t.accent)),
    ]));
    if let Some(error) = &dialog.error {
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(t.error),
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Enter submit pasted URL · Ctrl+Y copy URL · Esc cancel",
        muted,
    )));
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(frame(app, "Sign in to ChatGPT")),
        rect,
    );
}

fn waiting(callback: bool) -> &'static str {
    if callback {
        "Waiting for the browser… or paste the URL it was redirected to below."
    } else {
        "The server cannot receive the browser's redirect: after approving, paste the URL the browser was redirected to below."
    }
}
