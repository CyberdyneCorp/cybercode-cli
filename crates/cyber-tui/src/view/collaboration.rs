//! Collaboration labels are plain text; online presence is observational and task
//! reports are attributed self-reports, never local verification evidence.
use crate::{
    app::App,
    collaboration::{Snapshot, Tab, text},
};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub(super) fn view(f: &mut Frame, app: &App, area: Rect) {
    let state = &app.collaboration;
    let theme = &app.theme;
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Project collaboration · Tab: people / conversation / tasks · Esc: close ")
        .style(Style::default().fg(theme.text).bg(theme.background));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut lines = vec![Line::from(format!(
        "{:?}{}",
        state.tab,
        if state.loading {
            " · refreshing…"
        } else {
            ""
        }
    ))];
    if let Some(error) = &state.error {
        lines.push(Line::from(format!("Unavailable: {error}")));
        lines.push(Line::from(
            "Check cyber whoami and cyber collab status; r retries.",
        ));
    } else if let Some(snapshot) = &state.snapshot {
        body(snapshot, state.tab, state.selected, &mut lines);
    } else {
        lines.push(Line::from("Loading authenticated project state…"));
    }
    if let Some(editor) = &state.editor {
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "{} · Enter: send · Esc: cancel",
            editor.title
        )));
        lines.push(Line::from(format!("> {}", editor.input)));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn body(snapshot: &Snapshot, tab: Tab, selected: usize, lines: &mut Vec<Line<'static>>) {
    if snapshot.status["enabled"] != true {
        lines.push(Line::from(
            "Collaboration is off. Enable with cyber features enable collaboration.",
        ));
        return;
    }
    let Some(project) = snapshot.project() else {
        lines.push(Line::from(
            "No project binding. Sign in with cyber login, then cyber collab bind.",
        ));
        return;
    };
    lines.push(Line::from(format!(
        "Project {} · {} · Incoming agent messages require receiver consent in /messages",
        crate::messaging::clean(project),
        if snapshot.writable() {
            "member controls"
        } else {
            "read only"
        }
    )));
    if let Some(notice) = &snapshot.notice {
        lines.push(Line::from(notice.clone()));
    }
    for receipt in snapshot.status["receipts"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .take(3)
    {
        lines.push(Line::from(format!(
            "Operation {} · {} {}",
            text(receipt, "operation_id"),
            text(receipt, "state"),
            text(receipt, "detail")
        )));
    }
    match tab {
        Tab::People => people(snapshot, selected, lines),
        Tab::Conversation => conversation(snapshot, selected, lines),
        Tab::Tasks => tasks(snapshot, selected, lines),
    }
}

fn marker(index: usize, selected: usize) -> &'static str {
    if index == selected { ">" } else { " " }
}

fn people(snapshot: &Snapshot, selected: usize, lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(
        "↑/↓: select · d: direct human message · r: refresh",
    ));
    for (i, member) in snapshot.members.iter().enumerate() {
        lines.push(Line::from(format!(
            "{} {} [human, {}] · {} · {}",
            marker(i, selected),
            text(member, "name"),
            text(member, "kind"),
            text(member, "role"),
            if member["active"] == true {
                "active membership"
            } else {
                "revoked"
            }
        )));
    }
    for (i, agent) in snapshot.agents.iter().enumerate() {
        let owner = snapshot.participant(&serde_json::json!({"kind":"human","id":agent["owner"]}));
        lines.push(Line::from(format!(
            "{} {} [agent] · owner {} · {}",
            marker(i + snapshot.members.len(), selected),
            text(agent, "name"),
            owner,
            if agent["online"] == true {
                "online (observed)"
            } else {
                "offline"
            }
        )));
    }
}

fn conversation(snapshot: &Snapshot, selected: usize, lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(
        "n: project message · r: refresh · Direct messages are visible only to their participants.",
    ));
    for (i, message) in snapshot
        .messages
        .iter()
        .enumerate()
        .skip(selected.saturating_sub(2))
    {
        let author = snapshot.participant(&message["author"]);
        let target = if message["target"]["kind"] == "direct" {
            format!(
                "direct → {}",
                snapshot.participant(&message["target"]["participant"])
            )
        } else {
            "project".into()
        };
        lines.push(Line::from(format!(
            "{} {author} · {target}: {}",
            marker(i, selected),
            text(message, "text")
        )));
    }
}

fn tasks(snapshot: &Snapshot, selected: usize, lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(
        "↑/↓: select · n: new · c: claim · d: done · b: blocked · r: refresh",
    ));
    for (i, task) in snapshot
        .tasks
        .iter()
        .enumerate()
        .skip(selected.saturating_sub(2))
    {
        let assignee = if task["assignee"].is_null() {
            "unassigned".into()
        } else {
            snapshot.participant(&task["assignee"])
        };
        lines.push(Line::from(format!(
            "{} [{}] {} · {} · revision {} / claim {} {}",
            marker(i, selected),
            text(task, "status"),
            text(task, "title"),
            assignee,
            task["revision"],
            task["claim_generation"],
            text(task, "assignee_state")
        )));
        for report in task["reports"].as_array().into_iter().flatten() {
            lines.push(Line::from(format!(
                "  Self-report by {}: {}",
                snapshot.participant(&report["author"]),
                text(report, "text")
            )));
            for reference in report["references"].as_array().into_iter().flatten() {
                lines.push(Line::from(format!(
                    "    {}: {} (self-reported)",
                    text(reference, "kind"),
                    text(reference, "value")
                )));
            }
        }
    }
}
