use super::{Phase, View};
use crate::{app::App, messaging::clean};
use cyber_server::runtime::{SnapshotRecoveryImage, SnapshotRecoveryState};
use ratatui::text::Line;

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let view = &app.rewind.recovery;
    if view.phase == Phase::Preparation {
        return super::preparation::lines(app);
    }
    let mut lines = vec![
        Line::from("SNAPSHOT RECOVERY · composer and conversation stay unchanged"),
        Line::from("Esc closes · PgUp/PgDn scroll · external effects remain retained"),
    ];
    if view.busy {
        lines.push(Line::from(
            "Waiting for owner server; closing does not cancel an admitted worker.",
        ));
    }
    if let Some(request) = &view.request {
        lines.push(Line::from(format!(
            "Retained request: {:?}",
            request.request_id
        )));
    }
    match view.phase {
        Phase::Preparation => unreachable!(),
        Phase::Journals => journals(view, &mut lines),
        Phase::Requests => requests(view, &mut lines),
        Phase::Review => review(view, &mut lines),
        Phase::Outcome => outcome(view, &mut lines),
    }
    lines
}
fn journals(view: &View, lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(
        "↑/↓ select journal · Enter discovers retained requests · R reloads · P preparations",
    ));
    if view.journals.is_empty() {
        lines.push(Line::from(
            "No retained journals. P opens preparation recovery; no fence is cleared.",
        ));
    }
    for (index, journal) in view
        .journals
        .iter()
        .enumerate()
        .skip(view.selected.saturating_sub(4))
        .take(9)
    {
        lines.push(Line::from(format!(
            "{} {} · filesystem complete={} · recorded progress {}/{} (not acknowledgement)",
            if index == view.selected { ">" } else { " " },
            clean(&journal.id),
            journal.filesystem_complete,
            journal.applied.len(),
            journal.paths.len()
        )));
    }
}
fn requests(view: &View, lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(format!(
        "Journal: {}",
        clean(view.journal.as_deref().unwrap_or("unavailable"))
    )));
    if !view.discovered {
        lines.push(Line::from(
            "Request discovery unavailable; R retries without starting another attempt.",
        ));
        return;
    }
    if view.requests.is_empty() {
        lines.push(Line::from("No retained request. K reviews keep; B reviews rollback. Neither resumes the failed rewind."));
    } else {
        lines.push(Line::from("Retained requests found. ↑/↓ select · Enter looks up · R refreshes discovery. Inspect existing outcomes before another attempt."));
    }
    for (index, request) in view
        .requests
        .iter()
        .enumerate()
        .skip(view.selected.saturating_sub(4))
        .take(9)
    {
        lines.push(Line::from(format!(
            "{} {:?} · {:?}",
            if index == view.selected { ">" } else { " " },
            request.request_id,
            request.action
        )));
    }
}
fn image(value: Option<&SnapshotRecoveryImage>) -> String {
    value.map_or_else(
        || "absent".into(),
        |image| {
            format!(
                "{:?} {} · {} bytes · mode {:o} · uid {} gid {} · native {:?}",
                image.kind,
                clean(&image.hash),
                image.bytes,
                image.mode,
                image.uid,
                image.gid,
                image.native_identity
            )
        },
    )
}
fn review(view: &View, lines: &mut Vec<Line<'static>>) {
    let Some(review) = &view.review else {
        return;
    };
    lines.push(Line::from(format!(
        "REVIEW {:?} · {} · filesystem complete={} · acknowledged={}",
        review.action,
        clean(&review.journal_id),
        review.filesystem_complete,
        review.acknowledged
    )));
    if let Some(origin) = &review.origin {
        lines.push(Line::from(format!(
            "Original Session {} · worker {} · Location {:?}",
            clean(&origin.session_id),
            clean(&origin.activity_id),
            origin.binding.directory
        )));
    } else {
        lines.push(Line::from("Original worker identity unavailable"));
    }
    for path in &review.paths {
        lines.push(Line::from(format!(
            "{:?} [{}] changes={}",
            path.path,
            clean(&path.state),
            path.changes
        )));
        for (label, image_value) in [
            ("current", &path.current),
            ("before", &path.recorded_before),
            ("after", &path.recorded_after),
            ("retained", &path.retained),
            ("proposed", &path.proposed),
        ] {
            lines.push(Line::from(format!(
                "  {label}: {}",
                image(image_value.as_ref())
            )));
        }
    }
    paths("Historical exclusions", review.exclusions.as_deref(), lines);
    paths(
        "Protected created parents",
        review.recorded_created_parents.as_deref(),
        lines,
    );
    for conflict in &review.conflicts {
        lines.push(Line::from(format!("Conflict: {conflict:?}")));
    }
    if let Some(hash) = &review.confirmation {
        lines.push(Line::from(format!(
            "Fingerprint {} · Enter explicitly confirms · R refreshes review",
            clean(hash)
        )));
    } else {
        lines.push(Line::from("Confirmation unavailable; retain evidence."));
    }
}
fn paths(label: &str, paths: Option<&[String]>, lines: &mut Vec<Line<'static>>) {
    match paths {
        None => lines.push(Line::from(format!(
            "{label}: unavailable historical evidence"
        ))),
        Some([]) => lines.push(Line::from(format!("{label}: none"))),
        Some(paths) => lines.extend(
            paths
                .iter()
                .map(|path| Line::from(format!("{label}: {path:?}"))),
        ),
    }
}
fn outcome(view: &View, lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(
        "R looks up this exact request; Enter never repeats source work.",
    ));
    let Some(outcome) = &view.outcome else {
        lines.push(Line::from("Outcome not observed. Refresh lookup, or reopen C recovery to discover the reserved request after restart."));
        return;
    };
    lines.push(Line::from(format!(
        "Runtime state: {:?} · native acknowledgement: {}",
        outcome.state,
        outcome
            .journal_acknowledged
            .map_or("not observed by this response", |value| if value {
                "observed"
            } else {
                "not acknowledged"
            })
    )));
    match outcome.state {
        SnapshotRecoveryState::Acknowledged => lines.push(Line::from("A explicitly reconciles native acknowledgement metadata; source files are never replayed.")),
        SnapshotRecoveryState::CancelledBeforeOrigin | SnapshotRecoveryState::CancelledBeforeEffects => lines.push(Line::from("This attempt was cancelled before effects. The original restore is unresolved; K/B requests a fresh reviewed keep/rollback attempt.")),
        SnapshotRecoveryState::ResolvedByAnotherRequest => lines.push(Line::from("This attempt was resolved by another owned request. K/B requests a fresh review of the original restore; no source work is replayed.")),
        _ => lines.push(Line::from("Unresolved native evidence remains fenced. Preparation cancellation requires its exact pre-effect proof; no force-clear is available.")),
    }
}
