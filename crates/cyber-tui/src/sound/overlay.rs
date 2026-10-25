//! `/sounds` (`tui` → Sound settings command): sounds on or off, the pack, the volume,
//! `when` and each event, with a preview per event. Every change applies at once and
//! is saved to `kv.json`.

use crossterm::event::{KeyCode, KeyEvent};

use super::{Alert, Choice, Outcome, Pack, When, config};
use crate::app::{Action, App, Overlay};
use crate::osc::BEL;

/// The overlay's selected row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SoundsView {
    pub selected: usize,
}

/// One row of the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Enabled,
    Pack,
    Volume,
    When,
    Event(Alert),
}

/// The setting rows, above one row per event.
const SETTINGS: [Row; 4] = [Row::Enabled, Row::Pack, Row::Volume, Row::When];

/// Every row: the settings, then each [`Alert`] in priority order.
pub const ROWS: [Row; SETTINGS.len() + Alert::ALL.len()] = {
    let mut rows = [Row::Enabled; SETTINGS.len() + Alert::ALL.len()];
    let mut i = 0;
    while i < rows.len() {
        rows[i] = if i < SETTINGS.len() {
            SETTINGS[i]
        } else {
            Row::Event(Alert::ALL[i - SETTINGS.len()])
        };
        i += 1;
    }
    rows
};

/// The volume moves in steps of this.
const VOLUME_STEP: u8 = 10;

impl App {
    /// `/sounds`, the palette entry and `sounds_view`; the current pack is rendered in
    /// the background so previews start at once.
    pub(crate) fn open_sounds(&mut self) {
        self.overlay = Overlay::Sounds(SoundsView::default());
        self.sounds.prerender();
    }

    /// Up/Down select; Space toggles; Left/Right change the pack, volume or `when`; `p`
    /// or Enter previews; Esc closes.
    pub(crate) fn sounds_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Overlay::Sounds(view) = &mut self.overlay else {
            return Vec::new();
        };
        let row = ROWS[view.selected.min(ROWS.len() - 1)];
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => view.selected = view.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                view.selected = (view.selected + 1).min(ROWS.len() - 1)
            }
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
            KeyCode::Char(' ') | KeyCode::Right => return self.change_sound(row, true),
            KeyCode::Left => return self.change_sound(row, false),
            KeyCode::Char('p') | KeyCode::Enter => self.preview_sound(row),
            _ => {}
        }
        Vec::new()
    }

    /// Move `row` to its next (`forward`) or previous value and save the choices.
    fn change_sound(&mut self, row: Row, forward: bool) -> Vec<Action> {
        let chosen = &mut self.sounds.chosen;
        match row {
            Row::Enabled => chosen.enabled = !chosen.enabled,
            Row::Pack => chosen.pack = cycle(&Pack::ALL, chosen.pack, forward),
            Row::Volume if forward => {
                chosen.volume = chosen.volume.saturating_add(VOLUME_STEP).min(100)
            }
            Row::Volume => chosen.volume = chosen.volume.saturating_sub(VOLUME_STEP),
            Row::When => chosen.when = cycle(&When::ALL, chosen.when, forward),
            // Back on, an event gets what the global file gives it (a pack or a file),
            // as after a restart.
            Row::Event(alert) => {
                let global = &self.sounds.global_events[alert.index()];
                let choice = chosen.event_mut(alert);
                *choice = match (choice.on(), global.on()) {
                    (true, _) => Choice::Off,
                    (false, true) => global.clone(),
                    (false, false) => Choice::On,
                };
            }
        }
        if matches!(row, Row::Pack | Row::Volume) {
            self.sounds.prerender();
        }
        vec![Action::SavePreference {
            key: "sounds",
            value: config::saved_value(&self.sounds.chosen),
        }]
    }

    /// Play the row's event, or `done` from the other rows, whatever the focus and the
    /// switches say.
    fn preview_sound(&mut self, row: Row) {
        let event = match row {
            Row::Event(alert) => alert,
            _ => Alert::Done,
        };
        if self.sounds.preview(event) == Outcome::Bell {
            self.write_terminal(BEL);
        }
    }
}

fn cycle<T: Copy + PartialEq>(all: &[T], current: T, forward: bool) -> T {
    let at = all.iter().position(|v| *v == current).unwrap_or(0);
    let next = if forward {
        (at + 1) % all.len()
    } else {
        (at + all.len() - 1) % all.len()
    };
    all[next]
}
