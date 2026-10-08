//! Pickers, the permission and question prompts, help and autocomplete.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, Completion, PermStep, Picker, QuestionForm};
use crate::model::Request;

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(4));
    let h = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn frame<'a>(app: &App, title: &str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.accent))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
}

fn selectable(app: &App, text: String, selected: bool) -> Line<'static> {
    let t = &app.theme;
    if selected {
        Line::from(Span::styled(
            format!("❯ {text}"),
            Style::default()
                .fg(t.accent)
                .bg(t.selection)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        Line::from(Span::styled(
            format!("  {text}"),
            Style::default().fg(t.text),
        ))
    }
}

fn picker_prompt(app: &App, p: &Picker) -> Line<'static> {
    let t = &app.theme;
    let search = vec![
        Span::styled("search: ", Style::default().fg(t.muted)),
        Span::raw(p.query.clone()),
    ];
    match &p.pending {
        Some(crate::app::SessionOp::Rename { title, .. }) => Line::from(vec![
            Span::styled("new title: ", Style::default().fg(t.warning)),
            Span::raw(title.clone()),
        ]),
        Some(crate::app::SessionOp::Delete { .. }) => Line::from(Span::styled(
            "delete this Session and its children? y to confirm",
            Style::default().fg(t.error),
        )),
        None if p.kind == crate::app::PickerKind::Sessions => {
            let mut spans = search;
            spans.push(Span::styled(
                "   ^R rename ^F fork ^A archive ^D delete",
                Style::default().fg(t.muted),
            ));
            Line::from(spans)
        }
        None => Line::from(search),
    }
}

pub fn picker(f: &mut Frame, app: &App, p: &Picker, area: Rect) {
    let rect = centered(area, 80, 20);
    let mut lines = vec![picker_prompt(app, p), Line::default()];
    let room = rect.height.saturating_sub(5) as usize;
    let first = p.selected.saturating_sub(room.saturating_sub(1));
    for (pos, &i) in p.filtered.iter().enumerate().skip(first).take(room) {
        let c = &p.items[i];
        let text = if c.detail.is_empty() {
            c.label.clone()
        } else {
            format!("{}  {}", c.label, c.detail)
        };
        lines.push(selectable(app, text, pos == p.selected));
    }
    if p.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matches",
            Style::default().fg(app.theme.muted),
        )));
    }
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(frame(app, &p.title)), rect);
}

pub fn permission(f: &mut Frame, app: &App, step: &PermStep, area: Rect) {
    let Some(Request::Permission {
        action,
        resources,
        patterns,
        metadata,
        ..
    }) = app.active_request()
    else {
        return;
    };
    let t = &app.theme;
    let rect = centered(area, 90, 24);
    let mut lines = Vec::new();
    if let Some(title) = app.active_request().and_then(Request::origin_title) {
        lines.push(Line::from(Span::styled(
            title.to_owned(),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::default());
    }
    if let Some(warning) = metadata["warning"].as_str() {
        lines.push(Line::from(Span::styled(
            warning.to_owned(),
            Style::default().fg(t.error).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::default());
    }
    let resource_lines: Vec<Line> = resources
        .iter()
        .take(6)
        .map(|r| Line::from(Span::styled(format!("  {r}"), Style::default().fg(t.text))))
        .collect();
    lines.extend(resource_lines);
    if action == "auto_override" {
        if let Some(reason) = metadata["classifier_reason"].as_str() {
            lines.push(Line::from(format!("Classifier blocked: {reason}")));
        }
        lines.push(Line::from(format!(
            "Tool: {}",
            metadata["tool"].as_str().unwrap_or_default()
        )));
        lines.push(Line::from(format!("Arguments: {}", metadata["input"])));
        lines.push(Line::default());
        let selected = match step {
            PermStep::Choose(i) => *i,
            _ => 0,
        };
        let mut controls = vec![
            selectable(app, "Confirm replay once (y)".into(), selected == 0),
            selectable(app, "Cancel (n)".into(), selected == 1),
        ];
        controls.push(Line::from("PgUp/PgDn review details · Home return to top"));
        f.render_widget(Clear, rect);
        let block = frame(app, "Approve blocked call once");
        let inner = block.inner(rect);
        let panels = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(inner);
        f.render_widget(block, rect);
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((app.permission_scroll, 0)),
            panels[0],
        );
        f.render_widget(Paragraph::new(controls), panels[1]);
        return;
    }
    if let Some(diff) = metadata["diff"].as_str() {
        lines.push(Line::default());
        for l in diff.lines().take(12) {
            let color = if l.starts_with('+') {
                t.success
            } else if l.starts_with('-') {
                t.error
            } else {
                t.muted
            };
            lines.push(Line::from(Span::styled(
                format!("  {l}"),
                Style::default().fg(color),
            )));
        }
    }
    lines.push(Line::default());
    match step {
        PermStep::Choose(i) => {
            for (n, label) in ["Allow once (y)", "Allow always… (a)", "Reject (n)"]
                .iter()
                .enumerate()
            {
                lines.push(selectable(app, (*label).into(), *i == n));
            }
        }
        PermStep::ConfirmAlways => {
            lines.push(Line::from(Span::styled(
                "Save these approvals for this project:",
                Style::default().fg(t.warning),
            )));
            for p in patterns {
                lines.push(Line::from(format!("  {action} {p}")));
            }
            lines.push(Line::from(Span::styled(
                "Enter to confirm · Esc to go back",
                Style::default().fg(t.muted),
            )));
        }
        PermStep::Feedback(text) => {
            lines.push(Line::from(Span::styled(
                "Tell the model what to do instead (optional), then Enter:",
                Style::default().fg(t.warning),
            )));
            lines.push(Line::from(format!("› {text}")));
        }
    }
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(frame(app, &format!("Permission: {action}"))),
        rect,
    );
}

pub fn question(f: &mut Frame, app: &App, form: &QuestionForm, area: Rect) {
    let Some(Request::Question { questions, .. }) = app.active_request() else {
        return;
    };
    let t = &app.theme;
    let rect = centered(area, 90, 22);
    let mut tabs: Vec<Span> = questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            Span::styled(
                format!(" {} ", q.header),
                if i == form.tab {
                    Style::default()
                        .fg(t.accent)
                        .add_modifier(Modifier::REVERSED)
                } else {
                    Style::default().fg(t.muted)
                },
            )
        })
        .collect();
    if questions.len() > 1 {
        tabs.push(Span::styled(
            " Review ",
            if form.tab == questions.len() {
                Style::default()
                    .fg(t.accent)
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(t.muted)
            },
        ));
    }
    let mut lines = vec![Line::from(tabs), Line::default()];
    if let Some(title) = app.active_request().and_then(Request::origin_title) {
        lines.insert(0, Line::default());
        lines.insert(
            0,
            Line::from(Span::styled(
                title.to_owned(),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            )),
        );
    }
    match questions.get(form.tab) {
        Some(q) => {
            lines.push(Line::from(Span::styled(
                q.question.clone(),
                Style::default().fg(t.text).add_modifier(Modifier::BOLD),
            )));
            for (i, (label, detail)) in q.options.iter().enumerate() {
                let mark = if form.chosen[form.tab].contains(&i) {
                    "[x] "
                } else if q.multi {
                    "[ ] "
                } else {
                    ""
                };
                let text = if detail.is_empty() {
                    format!("{mark}{label}")
                } else {
                    format!("{mark}{label} — {detail}")
                };
                lines.push(selectable(app, text, form.cursor == i && !form.typing));
            }
            if q.custom {
                let typed = form.custom[form.tab].clone().unwrap_or_default();
                let text = if form.typing || !typed.is_empty() {
                    format!("Your answer: {typed}")
                } else {
                    "Type your own answer".into()
                };
                lines.push(selectable(app, text, form.cursor == q.options.len()));
            }
        }
        None => {
            lines.push(Line::from("Review your answers, then Enter to submit:"));
            for (i, q) in questions.iter().enumerate() {
                let picked: Vec<&str> = form.chosen[i]
                    .iter()
                    .filter_map(|&o| q.options.get(o).map(|(l, _)| l.as_str()))
                    .collect();
                lines.push(Line::from(format!("  {}: {}", q.header, picked.join(", "))));
            }
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Enter select · Space toggle · Tab next · Esc dismiss",
        Style::default().fg(t.muted),
    )));
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(frame(app, "Question")),
        rect,
    );
}

pub fn cost(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = app.cost.lines().into_iter().map(Line::from).collect();
    let rect = centered(area, 90, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(frame(app, "Cost")), rect);
}

pub fn help(f: &mut Frame, app: &App, area: Rect) {
    let rows = [
        ("Enter", "send (steer while running)"),
        ("/hooks", "review definitions and exact checkout approvals"),
        ("/cost", "token classes, subtree cost and cache hit rate"),
        (
            "/hooks history",
            "page recorded hook outcomes without replay",
        ),
        ("/admissions", "inspect or cancel pending delegation"),
        ("Tab / Alt+Enter", "queue while running"),
        ("Shift+Enter, Ctrl+J", "newline"),
        ("Esc", "interrupt"),
        ("Shift+Tab", "cycle mode"),
        ("Ctrl+T / Ctrl+O", "reasoning / expand tool output"),
        ("Ctrl+G", "edit in $EDITOR"),
        (
            "Ctrl+X m / l / n / t / f",
            "model · sessions · new · theme · fork",
        ),
        ("/model /resume /new /mode /compact /theme", "commands"),
        ("/subtask <prompt> · /tasks · /stop", "background tasks"),
        ("/agent · /subagents", "browse child and parent threads"),
        ("@path  !cmd", "mention a file · run a shell command"),
        ("Alt+Up", "take back the last queued message"),
        ("PageUp / PageDown", "scroll"),
        ("Ctrl+C", "clear, or quit when empty"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("{k:<44}"), Style::default().fg(app.theme.accent)),
                Span::raw(*v),
            ])
        })
        .collect();
    let rect = centered(area, 80, rows.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(frame(app, "Keys")), rect);
}

pub fn completion(f: &mut Frame, app: &App, c: &Completion, composer: Rect) {
    if c.items.is_empty() {
        return;
    }
    let height = (c.items.len() as u16 + 2).min(14);
    let rect = Rect {
        x: composer.x + 1,
        y: composer.y.saturating_sub(height),
        width: composer.width.saturating_sub(2).min(70),
        height,
    };
    let lines: Vec<Line> = c
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let text = if item.detail.is_empty() {
                format!("{}{}", c.trigger, item.label)
            } else {
                format!("{}{}  {}", c.trigger, item.label, item.detail)
            };
            selectable(app, text, i == c.selected)
        })
        .collect();
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border)),
        ),
        rect,
    );
}

pub(super) fn confirm_bypass(f: &mut Frame, app: &App, area: Rect) {
    let rect = centered(area, 70, 6);
    let lines = vec![
        Line::from(Span::styled(
            "Tools can run without normal permission prompts.",
            Style::default().fg(app.theme.warning),
        )),
        Line::from(""),
        Line::from("y to enable bypass · n / Esc to cancel"),
    ];
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(frame(app, "Enable bypass mode?")),
        rect,
    );
}

pub(super) fn confirm_stop_tasks(f: &mut Frame, app: &App, area: Rect) {
    let rect = centered(area, 70, 6);
    let lines = vec![
        Line::from("Stop all running background tasks of this Session?"),
        Line::from("y / Enter to stop · n / Esc to cancel"),
    ];
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(frame(app, "Stop background tasks?")),
        rect,
    );
}

pub fn hook_history(f: &mut Frame, app: &App, area: Rect) {
    let rect = centered(area, 100, 25);
    let lines: Vec<Line> = app.hooks.lines().into_iter().map(Line::from).collect();
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .scroll((app.hooks.scroll, 0))
            .block(frame(
                app,
                "Hook execution history · R refresh · N next · ↑/↓ scroll · Esc close",
            )),
        rect,
    );
}

pub fn hook_definitions(f: &mut Frame, app: &App, area: Rect) {
    let rect = centered(area, 110, 30);
    let lines: Vec<Line> = app
        .hook_definitions
        .lines()
        .into_iter()
        .map(Line::from)
        .collect();
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .scroll((app.hook_definitions.scroll, 0))
            .block(frame(app, "Hook definitions · T approve · U revoke")),
        rect,
    );
}
