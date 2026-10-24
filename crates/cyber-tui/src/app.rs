//! TUI state and the pure part of its behavior: keys and server events become state
//! changes plus [`Action`]s that the runner performs through the API.

use std::collections::BTreeMap;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::composer::Composer;
use crate::fuzzy;
use crate::model::{Choice, Item, Queued, Request, Session};
use crate::theme::{Appearance, Catalog, Tokens};

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
    /// Put code on the clipboard (native tool, else OSC 52).
    Copy(String),
    Memory(crate::memory::Request),
    /// `/memory on|off|readonly`: `PUT /memory/settings`.
    SetMemory(cyber_core::memory::MemoryState),
    /// `/commit` and `/pr-description`: generated text, prefilled as a shell command.
    GitText(GitText),
    /// `/btw`: `POST /sessions/:id/side`.
    SideChat {
        session_id: String,
        question: String,
    },
    ReconcileAdmission(crate::admissions::Request),
    Admission {
        request: crate::admissions::Request,
        stop: bool,
    },
    Refresh,
    /// Read one sidebar source (`sidebar::Part`).
    LoadSidebar(crate::sidebar::Part),
    /// `r` on a failed MCP server in the sidebar.
    ReconnectMcp(String),
    LoadCost {
        generation: u64,
    },
    /// The Settings Status tab's sources.
    LoadStatus {
        generation: u64,
    },
    /// `POST /sessions/:id/recap`; `manual` for `/recap`, whose failures are shown.
    Recap {
        session_id: String,
        manual: bool,
    },
    /// The Usage tab's provider-reported windows (`GET /usage/limits`).
    LoadLimits {
        generation: u64,
    },
    /// The Usage tab's contributing factors (`GET /usage/insights`).
    LoadInsights {
        generation: u64,
        window: &'static str,
    },
    /// The Settings Stats tab (`GET /usage/stats`).
    LoadStats {
        generation: u64,
        period: &'static str,
        /// Only the current Location's project.
        project: bool,
        zone: Option<String>,
    },
    /// One file explorer level (`GET /files`).
    LoadDir {
        path: String,
        hidden: bool,
    },
    /// The explorer filter's server search.
    SearchFiles(String),
    /// A file's preview; an `ask` file raises the permission request first.
    LoadPreview(String),
    /// Save a Config tab preference to `kv.json`.
    SavePreference {
        key: &'static str,
        value: Value,
    },
    Prompt {
        text: String,
        delivery: &'static str,
        /// The draft's chips whose labels are in `text`, in label order.
        chips: Vec<crate::attachments::Chip>,
    },
    Shell(String),
    /// `& cmd`: start a user shell command as a background Job of the Session.
    BackgroundShell(String),
    /// Ctrl+B: move the running foreground command to the background.
    MoveToBackground,
    Command {
        name: String,
        arguments: String,
        /// A client-chosen message ID, so a bundled review's reviewer
        /// delegation is known before the command is posted.
        id: Option<String>,
    },
    /// Fetch the awaited review's delegation record to learn its Job.
    ReviewDelegation {
        session: String,
        delegation: String,
    },
    /// Start a run of the `review` Workflow for this Session's target.
    StartReview {
        arguments: String,
    },
    /// Fetch the awaited review run, which ended.
    ReviewRun {
        run: String,
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
    /// The picker over the Sessions linked to a pull request (`--from-pr`).
    LoadPrSessions(u64),
    /// Rename the current Session (`/rename`); the new title arrives as an event.
    RenameSession {
        id: String,
        title: String,
    },
    Open(String),
    NewSession,
    LoadModels,
    /// `/skills`: the Location's skills for the Session agent, with diagnostics.
    LoadSkills,
    /// Refetch `/commands`, as MCP prompt commands come and go with connections.
    LoadCommands,
    /// `/plugins` (`reload: false`) or `/reload-plugins`: the Location's plugins.
    LoadPlugins {
        reload: bool,
    },
    SwitchModel(String),
    SwitchMode(String),
    Fork,
    Subtask(String),
    ApproveAuto,
    Compact(Option<String>),
    FindFiles(String),
    LoadAgents,
    RemoveQueued(String),
    /// Release a held message to the inbox (`r` on its card or in `/messages`).
    ReleaseHeld(String),
    /// `a` in `/messages`: clear the Session's `non_interactive` refusal override.
    AcceptMessages,
    SaveTheme(String),
    /// Write `theme` to the project's `.cyber/tui.jsonc` (`p` in the theme switcher).
    SaveProjectTheme(String),
    Rename {
        id: String,
        title: String,
    },
    Archive(String),
    Delete(String),
    ForkSession(String),
    Editor(String),
    /// A Workflow monitor read or run operation.
    Workflow(crate::workflows::Request),
    /// `/loop` (`loops-scheduling`).
    Loop(crate::loops::LoopRequest),
    /// `/team`, `/team start|stop|resume` (`agent-teams`).
    Team(crate::team::Request),
    Collaboration(crate::collaboration::Request),
    /// `/goal` (`goals`).
    Goal(crate::goals::GoalRequest),
    /// `/tree` and `/label` (`tui` → Branch tree view).
    Branch(crate::branches::Request),
    /// `/login` and `/logout` (`tui` → Provider login commands).
    Login(crate::login::LoginRequest),
    /// `/export` (`tui` → Export command).
    Export(crate::export::Request),
    /// Open a file in `$VISUAL`/`$EDITOR` (`s` on a Workflow Run: its script).
    EditFile(String),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PickerKind {
    Admissions,
    Tasks,
    Children,
    Sessions,
    Models,
    Modes,
    Skills,
    Plugins,
    Loops,
    Goals,
    Login,
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

    /// Replace the items of an open picker, keeping what the user typed and, while it is
    /// still listed, the selected item.
    fn refresh(&mut self, items: Vec<Choice>) {
        let key = self.current().map(|choice| choice.key.clone());
        self.items = items;
        self.filter();
        if let Some(at) =
            key.and_then(|key| self.filtered.iter().position(|&i| self.items[i].key == key))
        {
            self.selected = at;
        }
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
    HookHistory,
    HookDefinitions,
    Memory,
    ConfirmBypass,
    ConfirmStopTasks,
    /// Validated findings of the last bundled review, most severe first.
    Findings,
    /// The patch of a sidebar Modified files row.
    FileDiff {
        index: usize,
        scroll: u16,
    },
    Palette(crate::palette::Palette),
    /// The Settings panel; its state is `App::settings`.
    Settings,
    /// The file explorer; its state is `App::explorer`.
    Files,
    /// The theme switcher.
    Themes(Box<crate::switcher::Switcher>),
    /// The Workflow monitor; its state is `App::workflows`.
    Workflows,
    /// `/rename`: the title being edited.
    Rename(String),
    /// The end of a background task's log, opened from `/tasks`.
    TaskLog {
        title: String,
        text: String,
        scroll: u16,
    },
    /// `/btw`: a side question and its answer.
    SideChat(crate::side_chat::SideChat),
    /// `/messages`: the Session's message history; its state is `App::messaging`.
    Messages,
    /// `/team`: the members and the task board; the team is `Sidebar::team`.
    Team,
    Collaboration,
    /// `/tree`: the branch tree; its state is `App::branches`.
    Tree,
    /// `/login`: a provider sign-in; its state is `App::login`.
    Login,
    /// `/export`: replace the existing file?
    ConfirmExport(crate::export::Request),
}

/// Text the small model drafts for the user (`vcs-integration` → Commit and PR text
/// generation); the result is prefilled as a reviewable shell command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitText {
    /// A commit message for the staged diff: `git commit -m …`.
    Commit,
    /// A PR for `base..HEAD` (empty: the default base): `gh pr create …`.
    PullRequest { base: String },
}

/// What the composer's first character makes of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Prompt,
    /// `!`: a user shell command.
    Shell,
    /// `&`: a user shell command started in the background.
    Background,
    /// `/`: a slash command.
    Command,
}

/// Bound actions whose surface is not drawn yet; they are kept out of help and the
/// palette.
pub const UNAVAILABLE: &[&str] = &[];

/// Status line fields shown by default (`tui.statusline.fields`).
pub const DEFAULT_FIELDS: [&str; 4] = ["cwd", "workflows", "context", "cost"];

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
    pub cost: crate::settings::cost::CostView,
    pub hooks: crate::hooks::View,
    pub hook_definitions: crate::hook_definitions::View,
    pub memory: crate::memory::View,
    pub review: crate::review::View,
    pub(crate) mode_selection: Option<String>,
    pub items: Vec<Item>,
    /// Text streaming in for assistant messages not yet durable.
    pub streaming: BTreeMap<String, String>,
    pub queued: Vec<Queued>,
    pub requests: Vec<Request>,
    shown_request: Option<String>,
    pub composer: Composer,
    /// The composer's chips and the clipboard (`tui` → Large paste and image input).
    pub attachments: crate::attachments::Attachments,
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
    /// The resolved tokens of the current theme.
    pub theme: Tokens,
    pub themes: Catalog,
    pub appearance: Appearance,
    /// `tui.accessibility.reduced_motion` or screen-reader mode: no animation frames.
    pub reduced_motion: bool,
    /// A fixed time for every frame (snapshot tests); `None` reads the real clock.
    pub clock: Option<Instant>,
    /// When the UI started, the origin of test clocks.
    #[cfg(test)]
    started: Instant,
    /// With [`App::clock`], every clock read moves on by this much: a real clock's later
    /// reads are later, which code reading the clock twice must tolerate.
    #[cfg(test)]
    pub clock_step: Option<std::time::Duration>,
    #[cfg(test)]
    clock_reads: std::cell::Cell<u32>,
    /// An OSC 11 background query waiting for its reply in the key stream.
    pub(crate) background_probe: Option<crate::theme::detect::Osc11Reply>,
    /// Rendered Markdown blocks and the streaming split of each message.
    pub(crate) markdown: crate::markdown::Cache,
    /// The terminal shows OSC 8 hyperlinks.
    pub hyperlinks: bool,
    /// The code block selected for copying (Ctrl+X y), counted from the top.
    pub code_select: Option<usize>,
    /// Links the last frame drew, for the OSC 8 pass after it.
    screen_links: std::sync::Mutex<Vec<crate::osc::ScreenLink>>,
    /// The last frame's click targets and the armed click.
    pub(crate) mouse: crate::mouse::Mouse,
    leader: Option<Instant>,
    /// The merged keybinding table.
    pub keymap: crate::keys::Keymap,
    /// `/commands` entries for the palette.
    pub server_commands: Vec<crate::palette::ServerCommand>,
    /// Model display names and agent colors.
    pub catalog: crate::model::Catalog,
    /// `tui.statusline.fields`.
    pub statusline_fields: Vec<String>,
    /// `tui.accessibility.screen_reader`: statuses as sentences.
    pub screen_reader: bool,
    /// A fixed UTC offset in minutes for clock times (snapshot tests); `None` is local.
    pub utc_offset_minutes: Option<i32>,
    /// The Session sidebar's data, focus and visibility.
    pub sidebar: crate::sidebar::Sidebar,
    /// `tui.welcome`.
    pub welcome: crate::view::welcome::Welcome,
    /// The home directory, shown as `~`.
    pub home: Option<String>,
    /// The last frame's width, so `<leader>b` knows whether it docks or opens a drawer.
    pub(crate) frame_width: std::sync::atomic::AtomicU16,
    /// The Session or its directory changed: the sidebar reloads.
    pub(crate) sidebar_stale: bool,
    /// The Settings panel's tab, scroll and data.
    pub settings: crate::settings::Settings,
    /// The Config tab's preferences with their sources.
    pub prefs: crate::prefs::Preferences,
    /// This TUI's version, compared with the server's.
    pub client_version: String,
    /// This client's build identity, compared with the server's in the Status tab.
    pub client_build: String,
    /// The file explorer's tree, filter and preview.
    pub explorer: crate::explorer::Explorer,
    pub quit: bool,
    pub focused: bool,
    /// The Session recap line.
    pub recap: crate::recap::RecapState,
    /// The theme switcher's filter, which `theme_cycle` also follows.
    pub theme_filter: crate::switcher::Filter,
    /// `Theme: <name>` from `theme_cycle`, shown in the status footer.
    pub announcement: Option<(String, Instant)>,
    /// The activity indicator's sources and frames.
    pub progress: crate::progress::Progress,
    /// The Workflow monitor's runs, views and the transcript opened from it.
    pub workflows: crate::workflows::WorkflowsState,
    /// Cross-session message records, held cards and the `/messages` view.
    pub messaging: crate::messaging::Messages,
    /// The Session's branch tree (the header's branch tag) and the `/tree` view.
    pub branches: crate::branches::Tree,
    /// The Session's loops, for quiet-iteration collapsing and the footer.
    pub loops: crate::loops::Loops,
    /// The team view's selection and the way back from an opened teammate.
    pub team: crate::team::TeamState,
    pub collaboration: crate::collaboration::PanelState,
    /// The Session's current Goal, for the footer.
    pub goals: crate::goals::Goals,
    /// The `/login` dialog.
    pub login: crate::login::LoginDialog,
    /// The runs the indicators show and the notices of their ends.
    pub watch: crate::workflow_watch::Watch,
    /// `tui.notifications`.
    pub notifications: crate::osc::Notifications,
    /// Attention notifications written, instead of the terminal.
    #[cfg(test)]
    pub(crate) attention_log: Vec<String>,
}

impl App {
    pub fn new(session: Session, history: Vec<String>, theme_name: &str) -> Self {
        let themes = Catalog::default();
        let appearance = Appearance::default();
        let theme = themes
            .get(theme_name)
            .resolve(appearance.light(), appearance.depth);
        Self {
            admissions: Default::default(),
            session,
            cost: Default::default(),
            hooks: Default::default(),
            hook_definitions: Default::default(),
            memory: Default::default(),
            review: Default::default(),
            mode_selection: None,
            items: Vec::new(),
            streaming: BTreeMap::new(),
            queued: Vec::new(),
            requests: Vec::new(),
            shown_request: None,
            composer: Composer::new(history),
            attachments: Default::default(),
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
            theme,
            themes,
            appearance,
            reduced_motion: false,
            clock: None,
            #[cfg(test)]
            started: Instant::now(),
            #[cfg(test)]
            clock_step: None,
            #[cfg(test)]
            clock_reads: std::cell::Cell::new(0),
            background_probe: None,
            markdown: Default::default(),
            hyperlinks: false,
            code_select: None,
            screen_links: Default::default(),
            mouse: Default::default(),
            leader: None,
            keymap: Default::default(),
            server_commands: Vec::new(),
            catalog: Default::default(),
            statusline_fields: DEFAULT_FIELDS.iter().map(|f| (*f).to_owned()).collect(),
            screen_reader: false,
            utc_offset_minutes: None,
            sidebar: Default::default(),
            welcome: Default::default(),
            home: None,
            frame_width: Default::default(),
            sidebar_stale: false,
            settings: Default::default(),
            prefs: Default::default(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            client_build: cyber_core::version::build_info().build.to_owned(),
            explorer: Default::default(),
            quit: false,
            focused: true,
            recap: Default::default(),
            theme_filter: Default::default(),
            announcement: None,
            progress: Default::default(),
            workflows: Default::default(),
            messaging: Default::default(),
            branches: Default::default(),
            loops: Default::default(),
            team: Default::default(),
            collaboration: Default::default(),
            goals: Default::default(),
            login: Default::default(),
            watch: Default::default(),
            notifications: Default::default(),
            #[cfg(test)]
            attention_log: Vec::new(),
        }
    }

    pub(crate) fn set_screen_links(&self, links: Vec<crate::osc::ScreenLink>) {
        *self
            .screen_links
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = links;
    }

    pub(crate) fn take_screen_links(&self) -> Vec<crate::osc::ScreenLink> {
        std::mem::take(
            &mut self
                .screen_links
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    /// Every code block in the transcript, top to bottom, as the view numbers them.
    pub fn code_blocks(&self) -> Vec<String> {
        let mut out = Vec::new();
        for item in &self.items {
            if let Item::Assistant { id, text, .. } = item {
                let shown = match self.streaming.get(id) {
                    Some(streaming) if text.is_empty() => streaming,
                    _ => text,
                };
                out.extend(crate::markdown::code_blocks(shown));
            }
        }
        for (_, text) in self.unlisted_streams() {
            out.extend(crate::markdown::code_blocks(text));
        }
        out
    }

    /// Streamed text whose assistant message is not listed yet. A listed message shows
    /// its own stream while its text is empty, so it must not be drawn twice.
    pub fn unlisted_streams(&self) -> impl Iterator<Item = (&String, &String)> {
        self.streaming.iter().filter(|(id, _)| {
            !self
                .items
                .iter()
                .any(|i| matches!(i, Item::Assistant { id: aid, .. } if aid == *id))
        })
    }

    /// Select the last code block (Ctrl+X y); `y` then copies it.
    pub fn select_code(&mut self) {
        match self.code_blocks().len() {
            0 => self.toast("No code block to copy"),
            n => self.code_select = Some(n - 1),
        }
    }

    /// The command palette's "Copy last code block".
    pub fn copy_last_code_block(&mut self) -> Vec<Action> {
        match self.code_blocks().pop() {
            Some(code) => vec![Action::Copy(code)],
            None => {
                self.toast("No code block to copy");
                Vec::new()
            }
        }
    }

    /// Keys while a code block is selected; `None` lets the key through after leaving
    /// the selection.
    fn code_select_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let selected = self.code_select?;
        let blocks = self.code_blocks();
        let last = blocks.len().checked_sub(1)?;
        let selected = selected.min(last);
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                self.code_select = None;
                return Some(vec![Action::Copy(blocks[selected].clone())]);
            }
            KeyCode::Up | KeyCode::Char('k') => self.code_select = Some(selected.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.code_select = Some((selected + 1).min(last)),
            KeyCode::Esc => self.code_select = None,
            _ => {
                self.code_select = None;
                return None;
            }
        }
        Some(Vec::new())
    }

    pub(crate) fn set_session(&mut self, session: Session) {
        if session.id != self.session.id || session.directory != self.session.directory {
            self.memory.forget_location();
            if matches!(self.overlay, Overlay::Memory) {
                self.overlay = Overlay::None;
            }
        }
        if session.id != self.session.id {
            self.forget_session_views();
            self.thread_session(&session.id);
            self.team_session(&session.id);
        }
        if session.id != self.session.id || self.mode_selection.as_deref() == Some(&session.mode) {
            self.mode_selection = None;
        }
        if session.id != self.session.id {
            self.sidebar.forget_session();
            self.sidebar_stale = true;
            if matches!(self.overlay, Overlay::FileDiff { .. }) {
                self.overlay = Overlay::None;
            }
        }
        if session.directory != self.session.directory {
            self.explorer.forget();
            if matches!(self.overlay, Overlay::Files) {
                self.overlay = Overlay::None;
            }
            self.sidebar.forget_location();
            self.sidebar_stale = true;
            self.hook_definitions.invalidate();
            if matches!(self.overlay, Overlay::HookDefinitions) {
                self.overlay = Overlay::None;
            }
            self.agents.clear();
            self.completion = None;
        }
        self.session = session;
    }

    /// Another Session is shown: views of the previous one close and forget it.
    fn forget_session_views(&mut self) {
        self.forget_settings();
        self.messaging = Default::default();
        self.collaboration.forget();
        if matches!(self.overlay, Overlay::Messages | Overlay::Collaboration) {
            self.overlay = Overlay::None;
        }
        self.progress.forget_session();
        self.watch.fed = None;
        self.cost.invalidate();
        self.hooks.invalidate();
        self.hook_definitions.invalidate();
        if matches!(
            self.overlay,
            Overlay::Settings | Overlay::HookHistory | Overlay::HookDefinitions
        ) {
            self.overlay = Overlay::None;
        }
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
        self.toast = Some((text.into(), self.now()));
    }

    /// The time a frame is drawn at.
    pub fn now(&self) -> Instant {
        let now = self.clock.unwrap_or_else(Instant::now);
        #[cfg(test)]
        if let Some(step) = self.clock_step {
            let reads = self.clock_reads.get();
            self.clock_reads.set(reads + 1);
            return now + step * reads;
        }
        now
    }

    /// When the UI started.
    #[cfg(test)]
    pub fn started(&self) -> Instant {
        self.started
    }

    /// Resolve the named theme for the current appearance.
    pub fn set_theme(&mut self, name: &str) {
        let light = self.appearance.light();
        self.theme = self.themes.get(name).resolve(light, self.appearance.depth);
    }

    /// Use these themes and this appearance, keeping the current theme's name.
    pub fn set_appearance(&mut self, themes: Catalog, appearance: Appearance) {
        let name = self.theme.name.clone();
        self.themes = themes;
        self.appearance = appearance;
        self.set_theme(&name);
    }

    /// Take the keys of an OSC 11 reply out of the key stream; other keys come back.
    pub fn probe_key(&mut self, key: KeyEvent) -> Option<KeyEvent> {
        use crate::theme::detect::{Feed, is_light};
        let now = self.now();
        let Some(probe) = self.background_probe.as_mut() else {
            return Some(key);
        };
        match probe.feed(key, now) {
            Feed::Pass(key) => {
                if !self
                    .background_probe
                    .as_ref()
                    .is_some_and(|p| p.waiting(now))
                {
                    self.background_probe = None;
                }
                Some(key)
            }
            Feed::Swallowed => None,
            Feed::Done(rgb) => {
                self.background_probe = None;
                if let Some(rgb) = rgb {
                    self.background_detected(is_light(rgb));
                }
                None
            }
        }
    }

    /// The terminal reported its background (an OSC 11 reply): re-resolve the theme.
    pub fn background_detected(&mut self, light: bool) {
        self.appearance.detected_light = Some(light);
        let name = self.theme.name.clone();
        self.set_theme(&name);
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
        if !(blocking && same_request)
            && self.active_request().is_some()
            && let Overlay::Themes(s) = &self.overlay
        {
            // A prompt replaces the switcher: its preview is not kept.
            let original = s.original.clone();
            self.set_theme(&original);
        }
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
        self.mouse.disarm();
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('c')
            && self.composer.is_empty()
            && matches!(self.overlay, Overlay::None)
        {
            return vec![Action::Quit];
        }
        match &self.overlay {
            Overlay::None => match self.thread_key(key).or_else(|| self.team_return_key(key)) {
                Some(actions) => actions,
                None => self.composer_key(key),
            },
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
                    if !self.memory.cancel_confirmation() && !self.memory.leave_editor() {
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
            Overlay::Help => {
                self.overlay = Overlay::None;
                Vec::new()
            }
            Overlay::Findings => {
                self.findings_key(key);
                Vec::new()
            }
            Overlay::Palette(_) => self.palette_key(key),
            Overlay::Settings => self.settings_key(key),
            Overlay::Files => self.explorer_key(key),
            Overlay::Themes(_) => self.switcher_key(key),
            Overlay::Workflows => self.workflows_key(key),
            Overlay::Rename(_) => self.rename_key(key),
            Overlay::Login => self.login_key(key),
            Overlay::ConfirmExport(_) => self.confirm_export_key(key),
            Overlay::FileDiff { .. } => {
                self.file_diff_key(key);
                Vec::new()
            }
            Overlay::TaskLog { .. } => {
                self.task_log_key(key);
                Vec::new()
            }
            Overlay::SideChat(_) => {
                self.side_key(key);
                Vec::new()
            }
            Overlay::Messages => self.messages_key(key),
            Overlay::Team => self.team_key(key),
            Overlay::Collaboration => self.collaboration_key(key),
            Overlay::Tree => self.tree_key(key),
        }
    }

    /// What the composer's input is, from its first character.
    pub fn input_mode(&self) -> InputMode {
        match self.composer.lines().first().and_then(|l| l.chars().next()) {
            Some('!') => InputMode::Shell,
            Some('&') => InputMode::Background,
            Some('/') => InputMode::Command,
            _ => InputMode::Prompt,
        }
    }

    pub fn open_palette(&mut self) {
        self.completion = None;
        let entries = crate::palette::entries(&self.keymap, &self.server_commands);
        self.overlay = Overlay::Palette(crate::palette::Palette::new(entries));
    }

    fn palette_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let action = self.keymap.action("palette", &key, false);
        let Overlay::Palette(palette) = &mut self.overlay else {
            return Vec::new();
        };
        match (action, key.code) {
            (Some("palette_close"), _) => self.overlay = Overlay::None,
            (Some("palette_next"), _) => palette.step(true),
            (Some("palette_previous"), _) => palette.step(false),
            (Some("palette_run"), _) => return self.run_palette(),
            (_, KeyCode::Backspace) => {
                palette.query.pop();
                palette.filter();
            }
            (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                palette.query.push(c);
                palette.filter();
            }
            _ => {}
        }
        Vec::new()
    }

    fn run_palette(&mut self) -> Vec<Action> {
        let Overlay::Palette(palette) = std::mem::replace(&mut self.overlay, Overlay::None) else {
            return Vec::new();
        };
        let Some(entry) = palette.current().cloned() else {
            return Vec::new();
        };
        match entry.run {
            crate::palette::Run::Slash(name) => self.slash(&name),
            crate::palette::Run::Compose(name) => {
                self.composer.set_text(&format!("/{name} "));
                Vec::new()
            }
            crate::palette::Run::Ui(ui) => self.run_ui(ui),
        }
    }

    fn run_ui(&mut self, ui: crate::palette::Ui) -> Vec<Action> {
        use crate::palette::Ui;
        match ui {
            Ui::Settings(tab) => return self.open_settings(tab),
            Ui::Theme => self.open_theme_switcher(false),
            Ui::CopyLastCode => return self.copy_last_code_block(),
            Ui::ToggleReasoning => self.show_reasoning = !self.show_reasoning,
            Ui::ToggleToolDetails => self.expand_tools = !self.expand_tools,
            Ui::ToggleTimestamps => self.timestamps = !self.timestamps,
            Ui::ToggleSidebar => self.toggle_sidebar(),
            Ui::RenameSession => return self.rename_command(""),
            Ui::SessionTree => return self.tree_command(),
            Ui::LabelBranch => self.composer.set_text("/label "),
            Ui::ExportSession => return self.export_command(""),
        }
        Vec::new()
    }

    /// `/cost` and `/usage`: the Settings Usage tab, at the Session section.
    fn open_usage(&mut self) -> Vec<Action> {
        self.open_settings(Some(crate::settings::Tab::Usage))
    }

    /// Run a global binding's action.
    fn run_binding(&mut self, action: &str) -> Vec<Action> {
        match action {
            "command_palette" => self.open_palette(),
            "settings" => return self.open_settings(None),
            "sidebar_toggle" => self.toggle_sidebar(),
            "sidebar_focus" => self.focus_sidebar(),
            "file_explorer" => return self.open_explorer(),
            "model_list" => return vec![Action::LoadModels],
            "session_list" => return vec![Action::LoadSessions],
            "session_rename" => return self.rename_command(""),
            "session_new" => return vec![Action::NewSession],
            "theme_list" => self.open_theme_switcher(false),
            "theme_cycle" => return self.cycle_theme(),
            "session_fork" => return vec![Action::Fork],
            "copy_code" => self.select_code(),
            "help" => self.overlay = Overlay::Help,
            "reasoning_toggle" => self.show_reasoning = !self.show_reasoning,
            "tool_details" => self.expand_tools = !self.expand_tools,
            "background_command" => return self.move_to_background(),
            "messages_view" => return self.open_messages(),
            _ => {}
        }
        Vec::new()
    }

    /// Ctrl+B: only a running Turn can have a foreground command to move.
    fn move_to_background(&mut self) -> Vec<Action> {
        if self.session.running {
            return vec![Action::MoveToBackground];
        }
        self.toast("No command is running".to_string());
        Vec::new()
    }

    /// Keys over a task's log.
    fn task_log_key(&mut self, key: KeyEvent) {
        let Overlay::TaskLog { scroll, .. } = &mut self.overlay else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
            KeyCode::PageDown => *scroll = scroll.saturating_add(10),
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.overlay = Overlay::None,
            _ => {}
        }
    }

    /// Show a task's log, scrolled to its end.
    pub fn open_task_log(&mut self, title: String, text: String) {
        let scroll = u16::try_from(text.lines().count().saturating_sub(20)).unwrap_or(u16::MAX);
        self.overlay = Overlay::TaskLog {
            title,
            text,
            scroll,
        };
    }

    fn findings_key(&mut self, key: KeyEvent) {
        let scroll = &mut self.review.scroll;
        match key.code {
            KeyCode::Up => *scroll = scroll.saturating_sub(1),
            KeyCode::Down => *scroll = scroll.saturating_add(1),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
            KeyCode::PageDown => *scroll = scroll.saturating_add(10),
            _ => self.overlay = Overlay::None,
        }
    }

    /// The server refused a command before admission (an invalid review target,
    /// an empty diff or a permission denial); nothing runs.
    pub fn refused(&mut self, command: &str, message: &str) {
        if command == "review" {
            self.review.refused();
        }
        self.toast(cyber_core::review::escape(message));
    }

    /// The awaited review's delegation record arrived.
    pub fn review_delegation(&mut self, record: &Value) {
        let outcome = self.review.delegation(record);
        self.review_outcome(outcome);
    }

    /// The review run was admitted.
    pub fn review_started(&mut self, run: &str) -> Vec<Action> {
        let outcome = self.review.run_started(run);
        self.review_outcome(outcome)
    }

    /// The awaited review run, fetched after it ended.
    pub fn review_run(&mut self, run: &Value) {
        let outcome = self.review.run_settled(run);
        self.review_outcome(outcome);
    }

    fn job_ended(&mut self, job: &Value) -> Vec<Action> {
        let outcome = self.review.job_ended(job);
        self.review_outcome(outcome)
    }

    fn review_outcome(&mut self, outcome: crate::review::Ended) -> Vec<Action> {
        match outcome {
            crate::review::Ended::Findings if matches!(self.overlay, Overlay::None) => {
                self.overlay = Overlay::Findings;
            }
            crate::review::Ended::Findings => {
                self.toast("Review findings are ready; close this view to see them");
            }
            crate::review::Ended::Failed(error) => self.toast(format!("review failed: {error}")),
            crate::review::Ended::Lookup {
                session,
                delegation,
            } => {
                return vec![Action::ReviewDelegation {
                    session,
                    delegation,
                }];
            }
            crate::review::Ended::FetchRun(run) => return vec![Action::ReviewRun { run }],
            crate::review::Ended::Unrelated => {}
        }
        Vec::new()
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
        if self.rename_paste(text) || self.login_paste(text) || self.collaboration_paste(text) {
            return;
        }
        if let Overlay::Memory = self.overlay {
            self.memory.paste(text);
            return;
        }
        if let Overlay::None = self.overlay {
            self.paste_into_composer(text);
        }
    }

    /// Dropped image or PDF paths and large texts become chips; other text is typed.
    fn paste_into_composer(&mut self, text: &str) {
        match self.attachments.paste(text) {
            crate::attachments::Pasted::Chips(labels, toast) => {
                self.composer.insert_str(&labels);
                if let Some(toast) = toast {
                    self.toast(toast);
                }
            }
            crate::attachments::Pasted::Text(text) => self.composer.insert_str(&text),
        }
    }

    /// `composer.paste_image`: attach the clipboard image.
    fn paste_image(&mut self) {
        match self.attachments.paste_image() {
            Ok(label) => self.composer.insert_str(&format!("{label} ")),
            Err(message) => self.toast(message),
        }
    }

    /// A prompt the server refused: the draft comes back with its chips, unless a new
    /// one was started.
    pub(crate) fn prompt_refused(
        &mut self,
        session_id: &str,
        text: &str,
        chips: Vec<crate::attachments::Chip>,
        message: String,
    ) {
        if session_id == self.session.id && self.composer.is_empty() {
            self.composer.set_text(text);
            if let Some(gone) = self.attachments.restore(chips) {
                self.toast(gone);
            }
        } else {
            crate::attachments::discard(&chips);
        }
        self.toast(message);
    }

    fn leader_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let started = self.leader.take()?;
        if started.elapsed().as_millis() > LEADER_TIMEOUT_MS {
            return None;
        }
        Some(match self.keymap.action("global", &key, true) {
            Some(action) => self.run_binding(action),
            None => Vec::new(),
        })
    }

    fn composer_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if let Some(actions) = self.leader_key(key) {
            return actions;
        }
        if self.sidebar.focus.is_some() && !self.sidebar_visible() {
            self.sidebar.focus = None;
        }
        if self.sidebar.focus.is_some() {
            if self.keymap.is_leader(&key) {
                self.leader = Some(Instant::now());
                return Vec::new();
            }
            return self.sidebar_key(key);
        }
        if let Some(actions) = self.code_select_key(key) {
            return actions;
        }
        if let Some(actions) = self.held_card_key(key) {
            return actions;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if key.code == KeyCode::Tab && self.composer.text() == "/" {
            self.composer.clear();
            self.open_palette();
            return Vec::new();
        }
        if self.completion.is_some()
            && let Some(actions) = self.completion_key(key)
        {
            return actions;
        }
        if self.keymap.is_leader(&key) {
            self.leader = Some(Instant::now());
            return Vec::new();
        }
        if let Some(action) = self.keymap.action("global", &key, false) {
            return self.run_binding(action);
        }
        let actions = match key.code {
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
            KeyCode::Esc if self.hide_recap() => Vec::new(),
            _ => self.edit_key(key, ctrl),
        };
        self.update_completion(actions)
    }

    fn edit_key(&mut self, key: KeyEvent, ctrl: bool) -> Vec<Action> {
        match self.keymap.action("composer", &key, false) {
            Some("history_previous") => self.composer.up(),
            Some("history_next") => self.composer.down(),
            Some("newline") => self.composer.newline(),
            Some("editor") => return vec![Action::Editor(self.composer.text())],
            Some("paste_image") => self.paste_image(),
            _ => return self.edit_unbound(key, ctrl),
        }
        Vec::new()
    }

    /// Editing keys `tui.keybinds.composer` does not rebind. Up and Down only move the
    /// cursor here, so a disabled history binding never recalls history.
    fn edit_unbound(&mut self, key: KeyEvent, ctrl: bool) -> Vec<Action> {
        match key.code {
            KeyCode::Char('w') if ctrl => self.composer.delete_word(),
            KeyCode::Char('u' | 'c') if ctrl => {
                self.composer.clear();
                self.attachments.clear();
            }
            KeyCode::Char('d') if ctrl && self.composer.is_empty() => return vec![Action::Quit],
            KeyCode::Char(c) if !ctrl => self.composer.insert(c),
            KeyCode::Backspace if self.attachments.backspace(&mut self.composer) => {}
            KeyCode::Backspace => self.composer.backspace(),
            KeyCode::Delete if self.attachments.delete(&mut self.composer) => {}
            KeyCode::Delete => self.composer.delete(),
            KeyCode::Left => self.composer.left(),
            KeyCode::Right => self.composer.right(),
            KeyCode::Home => self.composer.home(),
            KeyCode::End => self.composer.end(),
            KeyCode::Up => {
                self.composer.cursor_up();
            }
            KeyCode::Down => {
                self.composer.cursor_down();
            }
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            _ => {}
        }
        Vec::new()
    }

    /// Pull the newest queued input back into the composer for editing. Received
    /// messages are not the user's input and stay queued.
    fn take_back(&mut self) -> Vec<Action> {
        let Some(at) = self
            .queued
            .iter()
            .rposition(|row| !self.messaging.is_message(row))
        else {
            return Vec::new();
        };
        let last = self.queued.remove(at);
        // The draft is replaced, so its chips go too: a draft `[Image #1]` would
        // otherwise stay beside the taken-back input's own `[Image #1]`.
        self.attachments.clear();
        let text = self.attachments.take_back(&last.text, &last.media);
        self.composer.set_text(&text);
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
            // Enter on a command already typed in full runs it rather than completing it.
            KeyCode::Enter
                if completion.trigger == '/'
                    && completion
                        .items
                        .get(completion.selected)
                        .is_some_and(|choice| {
                            self.composer
                                .trigger()
                                .is_some_and(|(_, _, word)| word == choice.key)
                        }) =>
            {
                self.completion = None;
                return None;
            }
            KeyCode::Tab | KeyCode::Enter if !completion.items.is_empty() => {
                let choice = completion.items[completion.selected].clone();
                let (trigger, start) = (completion.trigger, completion.start);
                self.completion = None;
                if trigger == '@' && self.complete_media(start, &choice.key) {
                    return Some(Vec::new());
                }
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

    /// An `@` pick of an image or PDF becomes a chip in place of the mention; false for
    /// any other pick.
    fn complete_media(&mut self, start: usize, key: &str) -> bool {
        match self.attachments.mention(key, &self.session.directory) {
            Some(Ok(label)) => self.composer.complete(start, &format!("{label} ")),
            Some(Err(message)) => self.toast(message),
            None => return false,
        }
        true
    }

    fn submit(&mut self, delivery: &'static str) -> Vec<Action> {
        if self.composer.is_empty() {
            return Vec::new();
        }
        let draft = self.composer.text();
        let command = draft.starts_with(['&', '!', '/']);
        if command && crate::attachments::names_media(&draft, &self.attachments.chips) {
            // Commands and shells take text only: keep the draft so nothing is lost.
            self.toast("Images and PDFs cannot be sent to a command or shell");
            return Vec::new();
        }
        let mut text = self.composer.take();
        let chips = self.attachments.take(&text);
        self.completion = None;
        self.scroll = 0;
        if command {
            text = crate::attachments::expand_text(&text, &chips);
            crate::attachments::discard(&chips);
        }
        if let Some(command) = text.strip_prefix('&') {
            return vec![Action::BackgroundShell(command.trim().to_string())];
        }
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
        self.hide_recap();
        vec![Action::Prompt {
            text,
            delivery,
            chips,
        }]
    }

    /// `/memory [global]` opens the review panel; `/memory on|off|readonly` saves the
    /// toggle, which Sessions apply at their next Safe Boundary.
    fn memory_command(&mut self, args: &str) -> Vec<Action> {
        use cyber_core::memory::MemoryState;
        let scope = match args {
            "" => crate::memory::Scope::Project,
            "global" => crate::memory::Scope::Global,
            "on" => return vec![Action::SetMemory(MemoryState::On)],
            "off" => return vec![Action::SetMemory(MemoryState::Off)],
            "readonly" => return vec![Action::SetMemory(MemoryState::Readonly)],
            _ => {
                self.toast("Use /memory, /memory global or /memory on|off|readonly");
                return Vec::new();
            }
        };
        self.overlay = Overlay::Memory;
        vec![Action::Memory(self.memory.open(scope, &self.session))]
    }

    /// Built-in slash commands; anything else runs as a skill or custom command.
    fn slash(&mut self, line: &str) -> Vec<Action> {
        let (name, args) = line
            .split_once(' ')
            .map_or((line, ""), |(n, a)| (n, a.trim()));
        match name {
            "memory" => self.memory_command(args),
            "loop" => self.loop_command(args),
            "team" => self.team_command(args),
            "goal" => self.goal_command(args),
            "login" => self.login_command(args),
            "logout" => self.logout_command(args),
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
            "cost" | "usage" => self.open_usage(),
            "settings" => self.open_settings(None),
            "status" => self.open_settings(Some(crate::settings::Tab::Status)),
            "config" => self.open_settings(Some(crate::settings::Tab::Config)),
            "stats" => self.open_settings(Some(crate::settings::Tab::Stats)),
            "approve" if args.is_empty() => vec![Action::ApproveAuto],
            "approve" => {
                self.toast("Usage: /approve");
                Vec::new()
            }
            "admissions" => {
                let items = self.admissions.choices(&self.session.id);
                self.open_picker(
                    PickerKind::Admissions,
                    "Admissions · Enter inspect · Ctrl+R reconcile · Ctrl+S cancel",
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
            "skills" => vec![Action::LoadSkills],
            "plugins" => vec![Action::LoadPlugins { reload: false }],
            "reload-plugins" => vec![Action::LoadPlugins { reload: true }, Action::LoadCommands],
            "resume" | "sessions" => vec![Action::LoadSessions],
            "rename" => self.rename_command(args),
            "export" => self.export_command(args),
            "tree" => self.tree_command(),
            "label" => self.label_command(args),
            "btw" | "side" => self.side_command(args),
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
                self.open_theme_switcher(false);
                Vec::new()
            }
            "timestamps" => {
                self.timestamps = !self.timestamps;
                Vec::new()
            }
            "recap" => self.recap_command(),
            "commit" => vec![Action::GitText(GitText::Commit)],
            "pr-description" => vec![Action::GitText(GitText::PullRequest { base: args.into() })],
            "sidebar" => {
                self.toggle_sidebar();
                Vec::new()
            }
            "files" => self.open_explorer(),
            "workflows" => self.open_workflows(),
            "messages" => self.open_messages(),
            "collab" => self.open_collaboration(),
            "help" => {
                self.overlay = Overlay::Help;
                Vec::new()
            }
            "exit" | "quit" => vec![Action::Quit],
            "review" => match self.review.submit(&self.session.id, args) {
                Ok(crate::review::Submit::Command(id)) => vec![Action::Command {
                    name: name.into(),
                    arguments: args.into(),
                    id,
                }],
                Ok(crate::review::Submit::Workflow) => vec![Action::StartReview {
                    arguments: args.into(),
                }],
                Err(message) => {
                    self.toast(format!("review: {message}"));
                    Vec::new()
                }
            },
            _ => vec![Action::Command {
                name: name.into(),
                arguments: args.into(),
                id: None,
            }],
        }
    }

    pub(crate) fn load_cost(&mut self) -> Vec<Action> {
        vec![Action::LoadCost {
            generation: self.cost.start(&self.session.id),
        }]
    }

    pub fn open_picker(&mut self, kind: PickerKind, title: &str, items: Vec<Choice>) {
        self.overlay = Overlay::Picker(Picker::new(kind, title, items));
    }

    /// Open a picker, or refresh it in place when one of this kind is already open (a
    /// live reload must not reset the filter or the selection under the user).
    pub fn open_or_refresh_picker(&mut self, kind: PickerKind, title: &str, items: Vec<Choice>) {
        match &mut self.overlay {
            Overlay::Picker(picker) if picker.kind == kind && picker.pending.is_none() => {
                picker.refresh(items);
            }
            _ => self.open_picker(kind, title, items),
        }
    }

    fn picker_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if let Some(actions) = self.goal_panel_key(key) {
            return actions;
        }
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
            && key.code == KeyCode::Char('r')
        {
            return picker
                .current()
                .and_then(|choice| self.admissions.0.get(&choice.key))
                .filter(|entry| entry.status == "unknown" && !entry.busy)
                .map(|entry| vec![Action::ReconcileAdmission(entry.request.clone())])
                .unwrap_or_default();
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
        // `x` stops the selected task while no filter is typed; Ctrl+S always does.
        let stop_key = match key.code {
            KeyCode::Char('x') => picker.query.is_empty() && key.modifiers.is_empty(),
            KeyCode::Char('s') => key.modifiers.contains(KeyModifiers::CONTROL),
            _ => false,
        };
        if picker.kind == PickerKind::Tasks && stop_key {
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
            PickerKind::Tasks if choice.key.starts_with("run_") => {
                self.open_workflow_run(choice.key)
            }
            PickerKind::Tasks => vec![Action::OpenTask(choice.key)],
            PickerKind::Sessions | PickerKind::Children => vec![Action::Open(choice.key)],
            PickerKind::Models => vec![Action::SwitchModel(choice.key)],
            PickerKind::Modes => self.select_mode(&choice.key),
            // Plugin rows only inform; changes go through `cyber plugins`.
            PickerKind::Plugins => Vec::new(),
            // An active loop's row stops it; ended rows only inform.
            PickerKind::Login => self.start_login(),
            PickerKind::Loops if choice.key.is_empty() => Vec::new(),
            // A Goal row prefills `/goal edit` for it.
            PickerKind::Goals => vec![Action::Goal(crate::goals::GoalRequest::Edit {
                session_id: self.session.id.clone(),
                goal_id: Some(choice.key),
            })],
            PickerKind::Loops => vec![Action::Loop(crate::loops::LoopRequest::Stop {
                session_id: self.session.id.clone(),
                loop_id: Some(choice.key),
            })],
            // A user-invocable skill starts its command; other rows only inform.
            PickerKind::Skills => {
                if !choice.key.is_empty() {
                    self.composer.set_text(&format!("/{} ", choice.key));
                }
                Vec::new()
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
        // A preview waiting for this approval brings the explorer back.
        if matches!(self.overlay, Overlay::None) && self.explorer.awaiting.is_some() {
            self.overlay = Overlay::Files;
        }
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
        if relocates(&event.kind) {
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
        self.progress.observe(kind, self.now());
        self.title_event(kind, data);
        self.delivery_event(kind, data);
        let mut actions = if kind == "job.ended.1" {
            self.job_ended(data)
        } else if kind.starts_with("workflow.run.") {
            let outcome = self.review.run_event(kind, data);
            self.review_outcome(outcome)
        } else if kind.starts_with("session.prompt.admitted") {
            let message = data["message_id"].as_str().unwrap_or_default();
            let outcome = self.review.admitted(message);
            self.review_outcome(outcome)
        } else {
            Vec::new()
        };
        actions.extend(self.apply_event(kind, data));
        actions
    }

    fn apply_event(&mut self, kind: &str, data: &Value) -> Vec<Action> {
        match kind {
            "server.connected" => {
                self.sidebar.connection = crate::sidebar::Connection::Connected;
                let mut actions = vec![Action::Refresh];
                actions.extend(crate::sidebar::Part::ALL.map(Action::LoadSidebar));
                actions.extend(self.workflows_connected());
                actions.extend(self.watch_reload());
                actions
            }
            "session.diff.1" => {
                let mut actions = vec![
                    Action::Refresh,
                    Action::LoadSidebar(crate::sidebar::Part::Files),
                ];
                actions.extend(self.refresh_explorer());
                actions
            }
            kind if relocates(kind) => {
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
                    self.attention("cyber: the session is idle");
                }
                // Language servers start as files are read, and the Turn may have changed
                // the working tree.
                let mut actions = vec![
                    Action::Refresh,
                    Action::LoadSidebar(crate::sidebar::Part::Lsp),
                    Action::LoadSidebar(crate::sidebar::Part::Vcs),
                ];
                actions.extend(self.refresh_explorer());
                actions
            }
            "session.usage" => {
                if let Some(meter) = crate::model::ContextMeter::parse(&data["context"]) {
                    self.session.context = Some(meter);
                }
                Vec::new()
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

/// Durable events that move the Session to another Location (a rebound isolated
/// child, `enter_worktree`, `exit_worktree`); each carries `to.path`.
fn relocates(kind: &str) -> bool {
    matches!(
        kind,
        "session.worktree.rebound.1" | "session.worktree.entered.1" | "session.worktree.exited.1"
    )
}
