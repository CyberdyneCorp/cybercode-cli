//! TUI state and the pure part of its behavior: keys and server events become state
//! changes plus [`Action`]s that the runner performs through the API.

use std::collections::BTreeMap;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::composer::Composer;
use crate::fuzzy;
use crate::model::{Choice, Item, Queued, Request, Session};
use crate::theme::{self, Theme};

/// Modes Shift+Tab cycles through; the others are set with `/mode`.
const CYCLE_MODES: [&str; 4] = ["default", "accept-edits", "plan", "auto"];
pub const MODES: [&str; 6] = [
    "default",
    "accept-edits",
    "plan",
    "auto",
    "dont-ask",
    "bypass",
];
const LEADER_TIMEOUT_MS: u128 = 2000;

/// Work for the runner.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Memory(crate::memory::Request),
    Admission {
        request: crate::admissions::Request,
        stop: bool,
    },
    Refresh,
    LoadCost {
        generation: u64,
    },
    Prompt {
        text: String,
        delivery: &'static str,
    },
    Shell(String),
    Command {
        name: String,
        arguments: String,
    },
    Interrupt,
    Reply {
        request: String,
        body: Value,
    },
    Answer {
        request: String,
        body: Value,
    },
    LoadHookDefinitions {
        generation: u64,
    },
    ChangeHookTrust {
        generation: u64,
        digest: String,
        approve: bool,
    },
    LoadHookHistory {
        generation: u64,
        cursor: Option<String>,
    },
    LoadTasks,
    LoadChildren,
    OpenTask(String),
    StopTask(String),
    StopTasks,
    LoadSessions,
    Open(String),
    NewSession,
    LoadModels,
    SwitchModel(String),
    SwitchMode(String),
    Fork,
    Subtask(String),
    ApproveAuto,
    Compact(Option<String>),
    FindFiles(String),
    LoadAgents,
    RemoveQueued(String),
    SaveTheme(&'static str),
    Rename {
        id: String,
        title: String,
    },
    Archive(String),
    Delete(String),
    ForkSession(String),
    Editor(String),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PickerKind {
    Admissions,
    Tasks,
    Children,
    Sessions,
    Models,
    Themes,
    Modes,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub kind: PickerKind,
    pub title: String,
    pub query: String,
    pub items: Vec<Choice>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    /// A Session action awaiting input or confirmation.
    pub pending: Option<SessionOp>,
}

/// Session picker operations that need a second step.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionOp {
    Rename { id: String, title: String },
    Delete { id: String },
}

impl Picker {
    pub fn new(kind: PickerKind, title: &str, items: Vec<Choice>) -> Self {
        let mut p = Self {
            kind,
            title: title.into(),
            query: String::new(),
            filtered: Vec::new(),
            items,
            selected: 0,
            pending: None,
        };
        p.filter();
        p
    }

    fn filter(&mut self) {
        self.filtered = fuzzy::rank(&self.query, &self.items, |c| {
            format!("{} {}", c.label, c.detail)
        });
        self.selected = 0;
    }

    pub fn current(&self) -> Option<&Choice> {
        self.filtered.get(self.selected).map(|&i| &self.items[i])
    }
}

/// Steps of the permission prompt.
#[derive(Debug, Clone, PartialEq)]
pub enum PermStep {
    Choose(usize),
    ConfirmAlways,
    Feedback(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuestionForm {
    pub tab: usize,
    pub cursor: usize,
    pub chosen: Vec<Vec<usize>>,
    /// Custom answers being typed, per question.
    pub custom: Vec<Option<String>>,
    pub typing: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    None,
    Picker(Picker),
    Permission(PermStep),
    Question(QuestionForm),
    Help,
    Cost,
    HookHistory,
    HookDefinitions,
    Memory,
    ConfirmBypass,
    ConfirmStopTasks,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub trigger: char,
    pub start: usize,
    pub items: Vec<Choice>,
    pub selected: usize,
}

pub struct App {
    pub admissions: crate::admissions::Admissions,
    pub session: Session,
    pub cost: crate::cost::CostView,
    pub hooks: crate::hooks::View,
    pub hook_definitions: crate::hook_definitions::View,
    pub memory: crate::memory::View,
    pub(crate) mode_selection: Option<String>,
    pub items: Vec<Item>,
    /// Text streaming in for assistant messages not yet durable.
    pub streaming: BTreeMap<String, String>,
    pub queued: Vec<Queued>,
    pub requests: Vec<Request>,
    shown_request: Option<String>,
    pub composer: Composer,
    pub overlay: Overlay,
    pub completion: Option<Completion>,
    pub commands: Vec<Choice>,
    pub files: Vec<String>,
    pub agents: Vec<Choice>,
    pub show_reasoning: bool,
    pub expand_tools: bool,
    pub timestamps: bool,
    /// Lines scrolled up from the bottom.
    pub scroll: u16,
    pub permission_scroll: u16,
    pub toast: Option<(String, Instant)>,
    pub theme: Theme,
    leader: Option<Instant>,
    pub quit: bool,
    pub focused: bool,
}

impl App {
    pub fn new(session: Session, history: Vec<String>, theme_name: &str) -> Self {
        Self {
            admissions: Default::default(),
            session,
            cost: Default::default(),
            hooks: Default::default(),
            hook_definitions: Default::default(),
            memory: Default::default(),
            mode_selection: None,
            items: Vec::new(),
            streaming: BTreeMap::new(),
            queued: Vec::new(),
            requests: Vec::new(),
            shown_request: None,
            composer: Composer::new(history),
            overlay: Overlay::None,
            completion: None,
            commands: Vec::new(),
            files: Vec::new(),
            agents: Vec::new(),
            show_reasoning: false,
            expand_tools: false,
            timestamps: false,
            scroll: 0,
            permission_scroll: 0,
            toast: None,
            theme: theme::by_name(theme_name),
            leader: None,
            quit: false,
            focused: true,
        }
    }

    pub(crate) fn set_session(&mut self, session: Session) {
        if session.id != self.session.id || session.directory != self.session.directory {
            self.memory.forget_location();
            if matches!(self.overlay, Overlay::Memory) {
                self.overlay = Overlay::None;
            }
        }
        if session.id != self.session.id {
            self.cost.invalidate();
            self.hooks.invalidate();
            self.hook_definitions.invalidate();
            if matches!(
                self.overlay,
                Overlay::Cost | Overlay::HookHistory | Overlay::HookDefinitions
            ) {
                self.overlay = Overlay::None;
            }
        }
        if session.id != self.session.id || self.mode_selection.as_deref() == Some(&session.mode) {
            self.mode_selection = None;
        }
        if session.directory != self.session.directory {
            self.hook_definitions.invalidate();
            if matches!(self.overlay, Overlay::HookDefinitions) {
                self.overlay = Overlay::None;
            }
            self.agents.clear();
            self.completion = None;
        }
        self.session = session;
    }

    pub fn mode_label(&self) -> String {
        match &self.mode_selection {
            Some(mode) if mode != &self.session.mode => {
                let mut current = self.session.clone();
                current.pending_mode = None;
                format!("{} → {mode} (switching)", current.mode_label())
            }
            _ => self.session.mode_label(),
        }
    }

    fn request_mode(&mut self, mode: &str) -> Vec<Action> {
        self.mode_selection = Some(mode.into());
        vec![Action::SwitchMode(mode.into())]
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// The oldest request owned by or routed to the viewed Session.
    pub fn active_request(&self) -> Option<&Request> {
        self.requests
            .iter()
            .find(|r| r.visible_in(&self.session.id))
    }

    /// Show the prompt for a newly pending request.
    pub fn sync_overlay(&mut self) {
        let blocking = matches!(self.overlay, Overlay::Permission(_) | Overlay::Question(_));
        let request_id = self
            .active_request()
            .map(|request| request.id().to_string());
        let same_request = self.shown_request == request_id;
        if !same_request {
            self.permission_scroll = 0;
        }
        self.shown_request = request_id;
        match (self.active_request(), blocking && same_request) {
            (Some(Request::Permission { .. }), false) => {
                self.overlay = Overlay::Permission(PermStep::Choose(0))
            }
            (Some(Request::Question { questions, .. }), false) => {
                let n = questions.len();
                self.overlay = Overlay::Question(QuestionForm {
                    tab: 0,
                    cursor: 0,
                    chosen: vec![Vec::new(); n],
                    custom: vec![None; n],
                    typing: false,
                });
            }
            (None, _) if blocking => self.overlay = Overlay::None,
            _ => {}
        }
    }

    pub(crate) fn refresh_admissions(&mut self) {
        let items = self.admissions.choices(&self.session.id);
        let Overlay::Picker(picker) = &mut self.overlay else {
            return;
        };
        if picker.kind != PickerKind::Admissions {
            return;
        }
        let selected = picker.current().map(|choice| choice.key.clone());
        picker.items = items;
        picker.filter();
        if let Some(index) = picker
            .filtered
            .iter()
            .position(|&index| Some(&picker.items[index].key) == selected.as_ref())
        {
            picker.selected = index;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('c')
            && self.composer.is_empty()
            && matches!(self.overlay, Overlay::None)
        {
            return vec![Action::Quit];
        }
        match &self.overlay {
            Overlay::None => self.composer_key(key),
            Overlay::Picker(_) => self.picker_key(key),
            Overlay::Permission(_) => self.permission_key(key),
            Overlay::Question(_) => self.question_key(key),
            Overlay::ConfirmBypass => self.confirm_bypass_key(key),
            Overlay::ConfirmStopTasks => {
                self.overlay = Overlay::None;
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) {
                    vec![Action::StopTasks]
                } else {
                    Vec::new()
                }
            }
            Overlay::HookHistory => self.hook_history_key(key),
            Overlay::HookDefinitions => self.hook_definitions_key(key),
            Overlay::Memory => {
                if key.code == KeyCode::Esc {
                    if !self.memory.cancel_confirmation() {
                        self.memory.invalidate();
                        self.overlay = Overlay::None;
                    }
                    Vec::new()
                } else {
                    self.memory
                        .key(key, &self.session)
                        .map(Action::Memory)
                        .into_iter()
                        .collect()
                }
            }
            Overlay::Cost => match key.code {
                KeyCode::Char('r' | 'R') => self.load_cost(),
                KeyCode::Esc | KeyCode::Enter => {
                    self.overlay = Overlay::None;
                    Vec::new()
                }
                _ => Vec::new(),
            },
            Overlay::Help => {
                self.overlay = Overlay::None;
                Vec::new()
            }
        }
    }

    fn hook_definitions_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            KeyCode::Char('r' | 'R') => {
                return vec![Action::LoadHookDefinitions {
                    generation: self.hook_definitions.load(),
                }];
            }
            KeyCode::Up => self.hook_definitions.select(false),
            KeyCode::Down => self.hook_definitions.select(true),
            KeyCode::PageUp => {
                self.hook_definitions.scroll = self.hook_definitions.scroll.saturating_sub(5)
            }
            KeyCode::PageDown => {
                self.hook_definitions.scroll = self.hook_definitions.scroll.saturating_add(5).min(
                    self.hook_definitions
                        .lines()
                        .len()
                        .saturating_sub(1)
                        .min(u16::MAX as usize) as u16,
                )
            }
            KeyCode::Char('t' | 'T') => self.hook_definitions.confirm(true),
            KeyCode::Char('u' | 'U') => self.hook_definitions.confirm(false),
            KeyCode::Char('y' | 'Y') => {
                if let Some((generation, digest, approve)) = self.hook_definitions.change() {
                    return vec![Action::ChangeHookTrust {
                        generation,
                        digest,
                        approve,
                    }];
                }
            }
            KeyCode::Char('n' | 'N') => {
                self.hook_definitions.cancel_confirmation();
            }
            KeyCode::Esc if !self.hook_definitions.cancel_confirmation() => {
                self.hook_definitions.invalidate();
                self.overlay = Overlay::None;
            }
            _ => {}
        }
        Vec::new()
    }

    fn load_hook_history(&mut self, cursor: Option<String>) -> Vec<Action> {
        vec![Action::LoadHookHistory {
            generation: self.hooks.load(),
            cursor,
        }]
    }
    fn hook_history_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            KeyCode::Char('r' | 'R') => return self.load_hook_history(None),
            KeyCode::Char('n' | 'N') => {
                if let Some(cursor) = self.hooks.next() {
                    return self.load_hook_history(Some(cursor));
                }
            }
            KeyCode::Up => self.hooks.scroll = self.hooks.scroll.saturating_sub(1),
            KeyCode::Down => {
                self.hooks.scroll = self.hooks.scroll.saturating_add(1).min(
                    self.hooks
                        .lines()
                        .len()
                        .saturating_sub(1)
                        .min(u16::MAX as usize) as u16,
                )
            }
            KeyCode::Esc | KeyCode::Enter => {
                self.overlay = Overlay::None;
                self.hooks.invalidate();
            }
            _ => {}
        }
        Vec::new()
    }

    pub fn on_paste(&mut self, text: &str) {
        if let Overlay::None = self.overlay {
            self.composer.insert_str(text);
        }
    }

    fn leader_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let started = self.leader.take()?;
        if started.elapsed().as_millis() > LEADER_TIMEOUT_MS {
            return None;
        }
        Some(match key.code {
            KeyCode::Char('m') => vec![Action::LoadModels],
            KeyCode::Char('l') => vec![Action::LoadSessions],
            KeyCode::Char('n') => vec![Action::NewSession],
            KeyCode::Char('t') => {
                self.open_themes();
                Vec::new()
            }
            KeyCode::Char('f') => vec![Action::Fork],
            KeyCode::Char('h') | KeyCode::Char('?') => {
                self.overlay = Overlay::Help;
                Vec::new()
            }
            _ => Vec::new(),
        })
    }

    fn composer_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if let Some(actions) = self.leader_key(key) {
            return actions;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if self.completion.is_some()
            && let Some(actions) = self.completion_key(key)
        {
            return actions;
        }
        let actions = match key.code {
            KeyCode::Char('x') if ctrl => {
                self.leader = Some(Instant::now());
                Vec::new()
            }
            KeyCode::Enter
                if key.modifiers.contains(KeyModifiers::SHIFT) || alt && !self.session.running =>
            {
                self.composer.newline();
                Vec::new()
            }
            KeyCode::Enter if alt => self.submit("queue"),
            KeyCode::Enter => self.submit("steer"),
            KeyCode::Tab if self.session.running && !self.composer.is_empty() => {
                self.submit("queue")
            }
            KeyCode::BackTab => self.cycle_mode(),
            KeyCode::Up if alt && !self.queued.is_empty() => self.take_back(),
            KeyCode::Esc if self.admissions.pending(&self.session.id).is_some() => {
                vec![
                    self.admissions
                        .pending(&self.session.id)
                        .expect("pending")
                        .request
                        .action(true),
                ]
            }
            KeyCode::Esc if self.session.running => vec![Action::Interrupt],
            _ => self.edit_key(key, ctrl),
        };
        self.update_completion(actions)
    }

    fn edit_key(&mut self, key: KeyEvent, ctrl: bool) -> Vec<Action> {
        match key.code {
            KeyCode::Char('j') if ctrl => self.composer.newline(),
            KeyCode::Char('t') if ctrl => self.show_reasoning = !self.show_reasoning,
            KeyCode::Char('o') if ctrl => self.expand_tools = !self.expand_tools,
            KeyCode::Char('g') if ctrl => return vec![Action::Editor(self.composer.text())],
            KeyCode::Char('w') if ctrl => self.composer.delete_word(),
            KeyCode::Char('u') if ctrl => self.composer.clear(),
            KeyCode::Char('c') if ctrl => self.composer.clear(),
            KeyCode::Char('d') if ctrl && self.composer.is_empty() => return vec![Action::Quit],
            KeyCode::Char(c) if !ctrl => self.composer.insert(c),
            KeyCode::Backspace => self.composer.backspace(),
            KeyCode::Delete => self.composer.delete(),
            KeyCode::Left => self.composer.left(),
            KeyCode::Right => self.composer.right(),
            KeyCode::Home => self.composer.home(),
            KeyCode::End => self.composer.end(),
            KeyCode::Up => self.composer.up(),
            KeyCode::Down => self.composer.down(),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            _ => {}
        }
        Vec::new()
    }

    /// Pull the newest queued message back into the composer for editing.
    fn take_back(&mut self) -> Vec<Action> {
        let Some(last) = self.queued.pop() else {
            return Vec::new();
        };
        self.composer.set_text(&last.text);
        vec![Action::RemoveQueued(last.message_id)]
    }

    fn select_mode(&mut self, mode: &str) -> Vec<Action> {
        if mode == "bypass" && self.session.mode != "bypass" {
            self.overlay = Overlay::ConfirmBypass;
            Vec::new()
        } else {
            self.request_mode(mode)
        }
    }

    fn confirm_bypass_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                self.overlay = Overlay::None;
                self.request_mode("bypass")
            }
            KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                self.overlay = Overlay::None;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn cycle_mode(&mut self) -> Vec<Action> {
        let i = CYCLE_MODES
            .iter()
            .position(|m| *m == self.mode_selection.as_deref().unwrap_or(&self.session.mode))
            .map_or(0, |i| (i + 1) % CYCLE_MODES.len());
        self.request_mode(CYCLE_MODES[i])
    }

    /// Keep the autocomplete menu in step with the word at the cursor.
    fn update_completion(&mut self, mut actions: Vec<Action>) -> Vec<Action> {
        let Some((trigger, start, word)) = self.composer.trigger() else {
            self.completion = None;
            return actions;
        };
        let items: Vec<Choice> = if trigger == '/' {
            let ranked = fuzzy::rank(&word, &self.commands, |c| c.label.clone());
            ranked
                .into_iter()
                .take(12)
                .map(|i| self.commands[i].clone())
                .collect()
        } else {
            actions.push(Action::FindFiles(word.clone()));
            let mut choices = Vec::new();
            if self.composer.cursor().0 == 0
                && self
                    .composer
                    .text()
                    .chars()
                    .take(start)
                    .all(char::is_whitespace)
            {
                actions.push(Action::LoadAgents);
                choices.extend(self.agents.clone());
            }
            choices.extend(
                self.files
                    .iter()
                    .filter(|file| !choices.iter().any(|agent| agent.key == **file))
                    .map(|file| Choice {
                        key: file.clone(),
                        label: file.clone(),
                        detail: String::new(),
                    })
                    .collect::<Vec<_>>(),
            );
            fuzzy::rank(&word, &choices, |choice| choice.label.clone())
                .into_iter()
                .take(20)
                .map(|i| choices[i].clone())
                .collect()
        };
        let selected = self
            .completion
            .as_ref()
            .filter(|c| c.trigger == trigger)
            .map_or(0, |c| c.selected.min(items.len().saturating_sub(1)));
        self.completion = Some(Completion {
            trigger,
            start,
            items,
            selected,
        });
        actions
    }

    pub(crate) fn set_agents(&mut self, agents: Vec<Choice>) {
        self.agents = agents;
        // Rebuild the menu locally; do not redispatch completion lookups.
        let _ = self.update_completion(Vec::new());
    }

    fn completion_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let completion = self.completion.as_mut()?;
        match key.code {
            KeyCode::Up => completion.selected = completion.selected.saturating_sub(1),
            KeyCode::Down => {
                completion.selected =
                    (completion.selected + 1).min(completion.items.len().saturating_sub(1))
            }
            KeyCode::Esc => self.completion = None,
            KeyCode::Tab | KeyCode::Enter if !completion.items.is_empty() => {
                let choice = completion.items[completion.selected].clone();
                let (trigger, start) = (completion.trigger, completion.start);
                self.completion = None;
                let name = if trigger == '@'
                    && !choice.detail.is_empty()
                    && choice
                        .key
                        .chars()
                        .any(|c| c.is_whitespace() || matches!(c, '"' | '\\'))
                {
                    serde_json::to_string(&choice.key).expect("string serialization")
                } else {
                    choice.key
                };
                let text = format!("{trigger}{name} ");
                self.composer.complete(start, &text);
                return Some(Vec::new());
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    fn submit(&mut self, delivery: &'static str) -> Vec<Action> {
        if self.composer.is_empty() {
            return Vec::new();
        }
        let text = self.composer.take();
        self.completion = None;
        self.scroll = 0;
        if let Some(command) = text.strip_prefix('!') {
            return vec![Action::Shell(command.trim().to_string())];
        }
        if let Some(rest) = text.strip_prefix('/') {
            return self.slash(rest.trim());
        }
        let delivery = if self.session.running {
            delivery
        } else {
            "steer"
        };
        vec![Action::Prompt { text, delivery }]
    }

    /// Built-in slash commands; anything else runs as a skill or custom command.
    fn slash(&mut self, line: &str) -> Vec<Action> {
        let (name, args) = line
            .split_once(' ')
            .map_or((line, ""), |(n, a)| (n, a.trim()));
        match name {
            "memory" if args.is_empty() || args == "global" => {
                self.overlay = Overlay::Memory;
                let scope = if args == "global" {
                    crate::memory::Scope::Global
                } else {
                    crate::memory::Scope::Project
                };
                vec![Action::Memory(self.memory.open(scope, &self.session))]
            }
            "memory" => {
                self.toast("Use /memory or /memory global");
                Vec::new()
            }
            "hooks" if args == "history" => {
                self.overlay = Overlay::HookHistory;
                self.load_hook_history(None)
            }
            "hooks" if args.is_empty() => {
                self.overlay = Overlay::HookDefinitions;
                vec![Action::LoadHookDefinitions {
                    generation: self.hook_definitions.load(),
                }]
            }
            "hooks" => {
                self.toast("Use /hooks to review definitions or /hooks history for receipts");
                Vec::new()
            }
            "cost" => {
                self.overlay = Overlay::Cost;
                self.load_cost()
            }
            "approve" if args.is_empty() => vec![Action::ApproveAuto],
            "approve" => {
                self.toast("Usage: /approve");
                Vec::new()
            }
            "admissions" => {
                let items = self.admissions.choices(&self.session.id);
                self.open_picker(
                    PickerKind::Admissions,
                    "Admissions · Enter inspect · Ctrl+S cancel",
                    items,
                );
                Vec::new()
            }
            "agent" | "subagents" => vec![Action::LoadChildren],
            "tasks" | "ps" => vec![Action::LoadTasks],
            "stop" => {
                self.overlay = Overlay::ConfirmStopTasks;
                Vec::new()
            }
            "model" | "models" => vec![Action::LoadModels],
            "resume" | "sessions" => vec![Action::LoadSessions],
            "new" | "clear" => vec![Action::NewSession],
            "fork" => vec![Action::Fork],
            "subtask" if !args.is_empty() => vec![Action::Subtask(args.into())],
            "subtask" => {
                self.toast("Usage: /subtask <prompt>");
                Vec::new()
            }
            "compact" => vec![Action::Compact(
                (!args.is_empty()).then(|| args.to_string()),
            )],
            "mode" if MODES.contains(&args) => self.select_mode(args),
            "mode" => {
                let items = MODES
                    .iter()
                    .map(|m| Choice {
                        key: (*m).into(),
                        label: (*m).into(),
                        detail: String::new(),
                    })
                    .collect();
                self.overlay = Overlay::Picker(Picker::new(PickerKind::Modes, "Mode", items));
                Vec::new()
            }
            "theme" => {
                self.open_themes();
                Vec::new()
            }
            "timestamps" => {
                self.timestamps = !self.timestamps;
                Vec::new()
            }
            "help" => {
                self.overlay = Overlay::Help;
                Vec::new()
            }
            "exit" | "quit" => vec![Action::Quit],
            _ => vec![Action::Command {
                name: name.into(),
                arguments: args.into(),
            }],
        }
    }

    fn load_cost(&mut self) -> Vec<Action> {
        vec![Action::LoadCost {
            generation: self.cost.start(&self.session.id),
        }]
    }

    fn open_themes(&mut self) {
        let items = theme::THEMES
            .iter()
            .map(|t| Choice {
                key: t.name.into(),
                label: t.name.into(),
                detail: String::new(),
            })
            .collect();
        self.overlay = Overlay::Picker(Picker::new(PickerKind::Themes, "Theme", items));
    }

    pub fn open_picker(&mut self, kind: PickerKind, title: &str, items: Vec<Choice>) {
        self.overlay = Overlay::Picker(Picker::new(kind, title, items));
    }

    fn picker_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Overlay::Picker(picker) = &mut self.overlay else {
            return Vec::new();
        };
        if picker.kind == PickerKind::Children
            && key.code == KeyCode::Char('r')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return vec![Action::LoadChildren];
        }
        if picker.pending.is_some() {
            return session_op_key(picker, key);
        }
        if picker.kind == PickerKind::Admissions
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('s')
        {
            return picker
                .current()
                .and_then(|choice| self.admissions.0.get(&choice.key))
                .map(|entry| vec![entry.request.action(true)])
                .unwrap_or_default();
        }
        if picker.kind == PickerKind::Tasks
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('s')
        {
            return picker
                .current()
                .map(|choice| vec![Action::StopTask(choice.key.clone())])
                .unwrap_or_default();
        }
        if picker.kind == PickerKind::Sessions && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.session_action(key);
        }
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => {
                picker.selected = (picker.selected + 1).min(picker.filtered.len().saturating_sub(1))
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.filter();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.query.push(c);
                picker.filter();
            }
            KeyCode::Enter => return self.pick(),
            _ => {}
        }
        if let Overlay::Picker(p) = &self.overlay
            && p.kind == PickerKind::Themes
            && let Some(choice) = p.current()
        {
            // Live preview.
            self.theme = theme::by_name(&choice.key);
        }
        Vec::new()
    }

    /// Ctrl+R rename, Ctrl+D delete, Ctrl+A archive, Ctrl+F fork in the Session picker.
    fn session_action(&mut self, key: KeyEvent) -> Vec<Action> {
        let Overlay::Picker(picker) = &mut self.overlay else {
            return Vec::new();
        };
        let Some(choice) = picker.current().cloned() else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Char('r') => {
                picker.pending = Some(SessionOp::Rename {
                    id: choice.key,
                    title: choice.label,
                })
            }
            KeyCode::Char('d') => picker.pending = Some(SessionOp::Delete { id: choice.key }),
            KeyCode::Char('a') => return vec![Action::Archive(choice.key)],
            KeyCode::Char('f') => {
                self.overlay = Overlay::None;
                return vec![Action::ForkSession(choice.key)];
            }
            _ => {}
        }
        Vec::new()
    }

    fn pick(&mut self) -> Vec<Action> {
        let Overlay::Picker(picker) = std::mem::replace(&mut self.overlay, Overlay::None) else {
            return Vec::new();
        };
        let Some(choice) = picker.current().cloned() else {
            return Vec::new();
        };
        match picker.kind {
            PickerKind::Admissions => self
                .admissions
                .0
                .get(&choice.key)
                .map(|entry| vec![entry.request.action(false)])
                .unwrap_or_default(),
            PickerKind::Tasks => vec![Action::OpenTask(choice.key)],
            PickerKind::Sessions | PickerKind::Children => vec![Action::Open(choice.key)],
            PickerKind::Models => vec![Action::SwitchModel(choice.key)],
            PickerKind::Modes => self.select_mode(&choice.key),
            PickerKind::Themes => {
                self.theme = theme::by_name(&choice.key);
                vec![Action::SaveTheme(self.theme.name)]
            }
        }
    }

    fn permission_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Some(Request::Permission { id, action, .. }) = self.active_request().cloned() else {
            self.overlay = Overlay::None;
            return Vec::new();
        };
        if action == "auto_override" {
            return self.auto_override_key(key, &id);
        }
        let Overlay::Permission(step) = &mut self.overlay else {
            return Vec::new();
        };
        let reply = |body: Value| {
            vec![Action::Reply {
                request: id.clone(),
                body,
            }]
        };
        match step {
            PermStep::Choose(i) => match key.code {
                KeyCode::Up => *i = i.saturating_sub(1),
                KeyCode::Down => *i = (*i + 1).min(2),
                KeyCode::Char('y') => return self.answered(reply(json!({ "reply": "once" }))),
                KeyCode::Char('a') => *step = PermStep::ConfirmAlways,
                KeyCode::Char('n') => *step = PermStep::Feedback(String::new()),
                KeyCode::Esc => return self.answered(reply(json!({ "reply": "reject" }))),
                KeyCode::Enter => match *i {
                    0 => return self.answered(reply(json!({ "reply": "once" }))),
                    1 => *step = PermStep::ConfirmAlways,
                    _ => *step = PermStep::Feedback(String::new()),
                },
                _ => {}
            },
            PermStep::ConfirmAlways => match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    return self.answered(reply(json!({ "reply": "always" })));
                }
                KeyCode::Esc | KeyCode::Char('n') => *step = PermStep::Choose(1),
                _ => {}
            },
            PermStep::Feedback(text) => match key.code {
                KeyCode::Enter => {
                    let message = (!text.trim().is_empty()).then(|| text.trim().to_string());
                    return self.answered(reply(json!({ "reply": "reject", "message": message })));
                }
                KeyCode::Esc => *step = PermStep::Choose(2),
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) => text.push(c),
                _ => {}
            },
        }
        Vec::new()
    }

    fn auto_override_key(&mut self, key: KeyEvent, id: &str) -> Vec<Action> {
        match key.code {
            KeyCode::PageDown => {
                self.permission_scroll = self.permission_scroll.saturating_add(8);
                return Vec::new();
            }
            KeyCode::PageUp => {
                self.permission_scroll = self.permission_scroll.saturating_sub(8);
                return Vec::new();
            }
            KeyCode::Home => {
                self.permission_scroll = 0;
                return Vec::new();
            }
            _ => {}
        }
        let Overlay::Permission(PermStep::Choose(selected)) = &mut self.overlay else {
            return Vec::new();
        };
        let approve = match key.code {
            KeyCode::Up => {
                *selected = 0;
                return Vec::new();
            }
            KeyCode::Down => {
                *selected = 1;
                return Vec::new();
            }
            KeyCode::Char('y') => true,
            KeyCode::Enter => *selected == 0,
            KeyCode::Esc | KeyCode::Char('n') => false,
            _ => return Vec::new(),
        };
        let body = if approve {
            json!({"reply":"once"})
        } else {
            json!({"reply":"reject","message":"Override cancelled"})
        };
        self.answered(vec![Action::Reply {
            request: id.into(),
            body,
        }])
    }

    /// Drop the answered request locally so the next one shows at once.
    fn answered(&mut self, actions: Vec<Action>) -> Vec<Action> {
        if let Some(id) = self.active_request().map(|r| r.id().to_string()) {
            self.requests.retain(|r| r.id() != id);
        }
        self.overlay = Overlay::None;
        self.sync_overlay();
        actions
    }

    fn question_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Some(Request::Question { id, questions, .. }) = self.active_request().cloned() else {
            self.overlay = Overlay::None;
            return Vec::new();
        };
        if key.code == KeyCode::Esc {
            return self.answered(vec![Action::Answer {
                request: id,
                body: json!({}),
            }]);
        }
        let Overlay::Question(form) = &mut self.overlay else {
            return Vec::new();
        };
        let review = questions.len() > 1 && form.tab == questions.len();
        if review {
            return match key.code {
                KeyCode::Enter => {
                    let answers = answers(&questions, form);
                    self.answered(vec![Action::Answer {
                        request: id,
                        body: json!({ "answers": answers }),
                    }])
                }
                KeyCode::Left | KeyCode::BackTab => {
                    form.tab -= 1;
                    Vec::new()
                }
                _ => Vec::new(),
            };
        }
        if question_input(form, &questions, key) {
            let answers = answers(&questions, form);
            return self.answered(vec![Action::Answer {
                request: id,
                body: json!({ "answers": answers }),
            }]);
        }
        Vec::new()
    }

    pub fn on_server_event(&mut self, event: &cyber_client::Event) -> Vec<Action> {
        if event.kind == "session.worktree.rebound.1" {
            if event.seq.is_none() {
                return vec![Action::Refresh];
            }
            if event.seq.is_some_and(|seq| seq <= self.session.seq) {
                return Vec::new();
            }
            let actions = self.on_event(&event.kind, &event.data);
            if let Some(seq) = event.seq
                && event.data["to"]["path"]
                    .as_str()
                    .is_some_and(|path| !path.is_empty())
            {
                self.session.seq = seq;
            }
            return actions;
        }
        self.on_event(&event.kind, &event.data)
    }

    /// Apply a server event; returns actions such as a refresh.
    pub fn on_event(&mut self, kind: &str, data: &Value) -> Vec<Action> {
        match kind {
            "server.connected" => vec![Action::Refresh],
            "session.worktree.rebound.1" => {
                if let Some(path) = data["to"]["path"].as_str().filter(|path| !path.is_empty()) {
                    let mut session = self.session.clone();
                    session.directory = path.into();
                    self.set_session(session);
                }
                vec![Action::Refresh]
            }
            "session.text.delta" => {
                let id = data["message_id"].as_str().unwrap_or_default().to_string();
                self.streaming
                    .entry(id)
                    .or_default()
                    .push_str(data["text"].as_str().unwrap_or_default());
                Vec::new()
            }
            "session.idle" => {
                self.session.running = false;
                if !self.focused {
                    notify("cyber: the session is idle");
                }
                vec![Action::Refresh]
            }
            "session.hook.notice" => {
                self.toast(format!(
                    "hook {}: {}",
                    data["hook_id"].as_str().unwrap_or_default(),
                    data["message"].as_str().unwrap_or_default()
                ));
                Vec::new()
            }
            "session.error" => {
                self.toast(format!(
                    "error: {}",
                    data["message"].as_str().unwrap_or_default()
                ));
                vec![Action::Refresh]
            }
            k if k.starts_with("session.step.started")
                || k.starts_with("session.prompt.promoted") =>
            {
                self.session.running = true;
                vec![Action::Refresh]
            }
            k if k.starts_with("job.")
                && matches!(&self.overlay,Overlay::Picker(picker) if picker.kind == PickerKind::Tasks) =>
            {
                vec![Action::LoadTasks, Action::Refresh]
            }
            k if is_durable(k) => vec![Action::Refresh],
            _ => Vec::new(),
        }
    }
}

/// Rename input or delete confirmation inside the Session picker.
fn session_op_key(picker: &mut Picker, key: KeyEvent) -> Vec<Action> {
    let Some(op) = picker.pending.as_mut() else {
        return Vec::new();
    };
    match (op, key.code) {
        (_, KeyCode::Esc) => picker.pending = None,
        (SessionOp::Rename { title, .. }, KeyCode::Char(c)) => title.push(c),
        (SessionOp::Rename { title, .. }, KeyCode::Backspace) => {
            title.pop();
        }
        (SessionOp::Rename { id, title }, KeyCode::Enter) if !title.trim().is_empty() => {
            let action = Action::Rename {
                id: id.clone(),
                title: title.trim().to_string(),
            };
            picker.pending = None;
            return vec![action];
        }
        (SessionOp::Delete { id }, KeyCode::Char('y')) => {
            let action = Action::Delete(id.clone());
            picker.pending = None;
            return vec![action];
        }
        (SessionOp::Delete { .. }, _) => picker.pending = None,
        _ => {}
    }
    Vec::new()
}

/// Durable event types end with a version number (`session.step.ended.1`).
fn is_durable(kind: &str) -> bool {
    kind.rsplit_once('.')
        .is_some_and(|(_, v)| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

/// Handle a key in a question tab; true when the form is complete.
fn question_input(
    form: &mut QuestionForm,
    questions: &[crate::model::QuestionSpec],
    key: KeyEvent,
) -> bool {
    let q = &questions[form.tab];
    let custom_row = q.options.len();
    if form.typing {
        let text = form.custom[form.tab].get_or_insert_with(String::new);
        match key.code {
            KeyCode::Char(c) => text.push(c),
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Enter => {
                form.typing = false;
                return advance(form, questions.len());
            }
            _ => {}
        }
        return false;
    }
    let rows = custom_row + usize::from(q.custom);
    match key.code {
        KeyCode::Up => form.cursor = form.cursor.saturating_sub(1),
        KeyCode::Down => form.cursor = (form.cursor + 1).min(rows.saturating_sub(1)),
        KeyCode::Tab | KeyCode::Right if questions.len() > 1 => {
            form.tab = (form.tab + 1).min(questions.len());
            form.cursor = 0;
        }
        KeyCode::Char(' ') if q.multi && form.cursor < custom_row => {
            toggle(&mut form.chosen[form.tab], form.cursor)
        }
        KeyCode::Enter if form.cursor == custom_row && q.custom => form.typing = true,
        KeyCode::Enter if q.multi => return advance(form, questions.len()),
        KeyCode::Enter => {
            form.chosen[form.tab] = vec![form.cursor];
            return advance(form, questions.len());
        }
        _ => {}
    }
    false
}

fn toggle(list: &mut Vec<usize>, i: usize) {
    match list.iter().position(|x| *x == i) {
        Some(p) => {
            list.remove(p);
        }
        None => list.push(i),
    }
}

/// Move to the next tab; a single question submits at once.
fn advance(form: &mut QuestionForm, total: usize) -> bool {
    if total == 1 {
        return true;
    }
    form.tab += 1;
    form.cursor = 0;
    false
}

fn answers(questions: &[crate::model::QuestionSpec], form: &QuestionForm) -> Vec<Vec<String>> {
    questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let mut picked: Vec<String> = form.chosen[i]
                .iter()
                .filter_map(|&o| q.options.get(o).map(|(l, _)| l.clone()))
                .collect();
            if let Some(text) = form.custom[i].as_ref().filter(|t| !t.trim().is_empty()) {
                picked.push(text.trim().to_string());
            }
            picked
        })
        .collect()
}

/// Terminal bell plus an OSC 9 desktop notification.
fn notify(text: &str) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = write!(out, "\x07\x1b]9;{text}\x07");
    let _ = out.flush();
}
