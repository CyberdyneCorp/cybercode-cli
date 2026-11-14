use super::{Phase, View};
use crate::app::App;
use ratatui::text::Line;

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let view = &app.rewind.recovery.preparation;
    let mut lines = vec![
        Line::from("PREPARATION RECOVERY · J journals · Esc closes"),
        Line::from(
            "Files, conversation and draft are preserved. Independent uncertainty remains protected.",
        ),
    ];
    if app.rewind.recovery.busy {
        lines.push(Line::from(
            "Waiting for owner server; closing does not undo admission.",
        ));
    }
    if let Some(request) = &view.request {
        lines.push(Line::from(format!(
            "Retained request: {:?}",
            request.request_id
        )));
    }
    match view.phase {
        Phase::Candidates => candidates(view, &mut lines),
        Phase::Review => review(view, &mut lines),
        Phase::Outcome => outcome(view, &mut lines),
    }
    lines
}
fn candidates(view: &View, lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(
        "↑/↓ select · Enter discovers a committed receipt before review · R reloads",
    ));
    if view.candidates.is_empty() {
        lines.push(Line::from(
            "No retained preparation candidates. No uncertainty is cleared.",
        ));
    }
    for (index, candidate) in view
        .candidates
        .iter()
        .enumerate()
        .skip(view.selected.saturating_sub(4))
        .take(9)
    {
        lines.push(Line::from(format!(
            "{} {:?} · status {:?} · lifetime evidence={}",
            if index == view.selected { ">" } else { " " },
            candidate.activity_id,
            candidate.status,
            candidate.lifetime_available
        )));
    }
}
fn review(view: &View, lines: &mut Vec<Line<'static>>) {
    let Some(review) = &view.review else {
        return;
    };
    lines.push(Line::from(format!(
        "Review {:?} · Location {:?}",
        review.activity_id, review.lifetime.owner.directory
    )));
    lines.push(Line::from(format!(
        "Lifetime v{} · native sequence {} · Session sequence {} · origin published={}",
        review.lifetime.version,
        review.native_sequence,
        review.session_sequence,
        review.origin.is_some()
    )));
    lines.push(Line::from(format!(
        "Acquired checkout digest: {:?}",
        review.checkout_resources_digest
    )));
    lines.push(Line::from(crate::messaging::clean(&review.warning)));
    lines.push(Line::from(
        "Enter cancels only this reviewed pre-effect preparation · R refreshes",
    ));
}
fn outcome(view: &View, lines: &mut Vec<Line<'static>>) {
    let Some(receipt) = &view.receipt else {
        lines.push(Line::from(
            "Outcome unknown. R looks up the retained request without repeating cancellation.",
        ));
        return;
    };
    lines.push(Line::from(format!(
        "Committed cancellation {:?} · Location {:?}",
        receipt.activity_id, receipt.lifetime.owner.directory
    )));
    lines.push(Line::from(format!(
        "Lifetime v{} · origin published={} · acquired checkout digest: {:?}",
        receipt.lifetime.version,
        receipt.origin.is_some(),
        receipt.checkout_resources_digest
    )));
    lines.push(Line::from(if view.reconciled { "Explicit acknowledgement reconciliation completed for this receipt." } else { "Receipt proves durable cancellation. Native acknowledgement is not established by this response alone." }));
    lines.push(Line::from("R looks up the same request · A reconciles native metadata · Enter never repeats cancellation"));
}
