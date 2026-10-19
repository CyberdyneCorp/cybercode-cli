//! The command palette (`tui` → Command palette): built-in TUI commands, the server's
//! custom commands, skills and MCP prompts, and UI actions, filtered by fuzzy match.

use crate::fuzzy;
use crate::keys::Keymap;

/// A command the server lists at `/commands`.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerCommand {
    pub name: String,
    pub description: String,
    /// `builtin`, `skill`, `command` or `mcp`.
    pub source: String,
    /// Arguments are expected, so the palette types `/name ` instead of running it.
    pub takes_arguments: bool,
}

/// What Enter does.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    /// Run `/name` as if typed.
    Slash(String),
    /// Type `/name ` into the composer for its arguments.
    Compose(String),
    Ui(Ui),
}

/// Actions that are not slash commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ui {
    /// The Settings panel on a tab, or on the last used one.
    Settings(Option<crate::settings::Tab>),
    Theme,
    CopyLastCode,
    ToggleReasoning,
    ToggleToolDetails,
    ToggleTimestamps,
    ToggleSidebar,
    RenameSession,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub description: String,
    /// The current binding (`ctrl+x m`); `None` when there is none or it is disabled.
    pub key: Option<String>,
    pub run: Run,
}

/// `(name, description, binding action, takes arguments)` of built-in TUI commands.
const BUILTINS: &[(&str, &str, Option<&str>, bool)] = &[
    ("help", "keys and commands", Some("help"), false),
    ("model", "switch model", Some("model_list"), false),
    (
        "sessions",
        "open another session",
        Some("session_list"),
        false,
    ),
    ("new", "start a new session", Some("session_new"), false),
    (
        "rename",
        "rename this session",
        Some("session_rename"),
        true,
    ),
    ("fork", "fork this session", Some("session_fork"), false),
    (
        "loop",
        "repeat a prompt on an interval (or self-paced); list, stop",
        None,
        true,
    ),
    (
        "btw",
        "ask a side question without adding it to the session",
        None,
        true,
    ),
    ("mode", "choose the permission mode", None, false),
    ("theme", "switch theme", Some("theme_list"), false),
    ("compact", "summarize older history", None, false),
    (
        "cost",
        "tokens, cost and cache hit rate of this session tree",
        None,
        false,
    ),
    (
        "settings",
        "status, preferences, usage and stats",
        Some("settings"),
        false,
    ),
    (
        "files",
        "browse and preview project files",
        Some("file_explorer"),
        false,
    ),
    (
        "status",
        "versions, session, model, context and servers",
        None,
        false,
    ),
    ("config", "edit TUI preferences", None, false),
    (
        "stats",
        "activity, streaks and per-model totals",
        None,
        false,
    ),
    (
        "memory",
        "review memory notes; /memory on|off|readonly",
        None,
        false,
    ),
    (
        "hooks",
        "review hook definitions and approvals",
        None,
        false,
    ),
    ("skills", "skills for this agent", None, false),
    ("tasks", "background tasks", None, false),
    ("agent", "child and parent threads", None, false),
    ("admissions", "pending delegations", None, false),
    ("review", "review the working tree changes", None, false),
    (
        "commit",
        "draft a commit message for the staged changes",
        None,
        false,
    ),
    (
        "pr-description",
        "draft a pull request title and body",
        None,
        true,
    ),
    ("subtask", "run a prompt as a background task", None, true),
    ("stop", "stop all background tasks", None, false),
    ("timestamps", "show or hide message times", None, false),
    ("recap", "summarize what this Session did", None, false),
    (
        "sidebar",
        "show or hide the sidebar",
        Some("sidebar_toggle"),
        false,
    ),
    (
        "workflows",
        "monitor and control Workflow Runs",
        None,
        false,
    ),
    (
        "messages",
        "messages to and from other Sessions; release held ones",
        Some("messages_view"),
        false,
    ),
    ("exit", "quit", None, false),
];

/// `(name, description, binding action, action)` of UI actions.
const ACTIONS: &[(&str, &str, Option<&str>, Ui)] = &[
    (
        "Settings",
        "status, preferences, usage and stats",
        Some("settings"),
        Ui::Settings(None),
    ),
    (
        "Settings: Status",
        "versions, session, model, context and servers",
        None,
        Ui::Settings(Some(crate::settings::Tab::Status)),
    ),
    (
        "Settings: Config",
        "edit TUI preferences",
        None,
        Ui::Settings(Some(crate::settings::Tab::Config)),
    ),
    (
        "Settings: Usage",
        "session usage and cost",
        None,
        Ui::Settings(Some(crate::settings::Tab::Usage)),
    ),
    (
        "Settings: Stats",
        "activity, streaks and per-model totals",
        None,
        Ui::Settings(Some(crate::settings::Tab::Stats)),
    ),
    (
        "Rename session",
        "change this session's title to resume it later",
        Some("session_rename"),
        Ui::RenameSession,
    ),
    (
        "Toggle sidebar",
        "show or hide the session sidebar",
        Some("sidebar_toggle"),
        Ui::ToggleSidebar,
    ),
    (
        "Theme…",
        "preview and pick a theme",
        Some("theme_list"),
        Ui::Theme,
    ),
    (
        "Copy last code block",
        "put the newest code block on the clipboard",
        Some("copy_code"),
        Ui::CopyLastCode,
    ),
    (
        "Toggle reasoning",
        "show or hide model reasoning",
        Some("reasoning_toggle"),
        Ui::ToggleReasoning,
    ),
    (
        "Toggle tool output",
        "expand or collapse long tool output",
        Some("tool_details"),
        Ui::ToggleToolDetails,
    ),
    (
        "Toggle timestamps",
        "show or hide message times",
        None,
        Ui::ToggleTimestamps,
    ),
];

/// Untrusted names and descriptions (skills, MCP prompts) on one line, without control
/// characters that could reach the terminal.
fn clean(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

/// Every entry: UI actions, built-ins, then server commands not shadowed by a built-in.
pub fn entries(keys: &Keymap, server: &[ServerCommand]) -> Vec<Entry> {
    let key = |action: Option<&str>| action.and_then(|a| keys.label("global", a));
    let mut out: Vec<Entry> = ACTIONS
        .iter()
        .map(|&(name, description, action, ui)| Entry {
            name: name.into(),
            description: description.into(),
            key: key(action),
            run: Run::Ui(ui),
        })
        .collect();
    out.extend(
        BUILTINS
            .iter()
            .map(|&(name, description, action, arguments)| Entry {
                name: format!("/{name}"),
                description: description.into(),
                key: key(action),
                run: if arguments {
                    Run::Compose(name.into())
                } else {
                    Run::Slash(name.into())
                },
            }),
    );
    for command in server {
        let name = clean(&command.name);
        if name.is_empty() || BUILTINS.iter().any(|b| b.0 == name) {
            continue;
        }
        let kind = match command.source.as_str() {
            "skill" => "skill",
            "mcp" => "MCP prompt",
            _ => "command",
        };
        out.push(Entry {
            name: format!("/{name}"),
            description: format!("{kind} · {}", clean(&command.description)),
            key: None,
            run: if command.takes_arguments {
                Run::Compose(name)
            } else {
                Run::Slash(name)
            },
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub query: String,
    pub entries: Vec<Entry>,
    pub filtered: Vec<usize>,
    pub selected: usize,
}

impl Palette {
    pub fn new(entries: Vec<Entry>) -> Self {
        let mut palette = Self {
            query: String::new(),
            filtered: Vec::new(),
            entries,
            selected: 0,
        };
        palette.filter();
        palette
    }

    pub fn filter(&mut self) {
        self.filtered = fuzzy::rank(&self.query, &self.entries, |e| {
            format!("{} {}", e.name, e.description)
        });
        self.selected = 0;
    }

    pub fn current(&self) -> Option<&Entry> {
        self.filtered.get(self.selected).map(|&i| &self.entries[i])
    }

    pub fn step(&mut self, down: bool) {
        let last = self.filtered.len().saturating_sub(1);
        self.selected = if down {
            (self.selected + 1).min(last)
        } else {
            self.selected.saturating_sub(1)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_commands_are_listed_cleanly_and_never_shadow_builtins() {
        let server = [
            ServerCommand {
                name: "deploy".into(),
                description: "ship it\u{1b}]52;c;x\u{7}\nsecond line".into(),
                source: "command".into(),
                takes_arguments: true,
            },
            ServerCommand {
                name: "review".into(),
                description: "bundled".into(),
                source: "builtin".into(),
                takes_arguments: false,
            },
            ServerCommand {
                name: "docs:search".into(),
                description: "find docs".into(),
                source: "mcp".into(),
                takes_arguments: false,
            },
        ];
        let entries = entries(&Keymap::default(), &server);
        let deploy = entries.iter().find(|e| e.name == "/deploy").unwrap();
        assert_eq!(deploy.description, "command · ship it]52;c;x");
        assert_eq!(deploy.run, Run::Compose("deploy".into()));
        assert_eq!(entries.iter().filter(|e| e.name == "/review").count(), 1);
        let mcp = entries.iter().find(|e| e.name == "/docs:search").unwrap();
        assert!(mcp.description.starts_with("MCP prompt"));
        let model = entries.iter().find(|e| e.name == "/model").unwrap();
        assert_eq!(model.key.as_deref(), Some("ctrl+x m"));
    }

    #[test]
    fn typing_filters_and_the_usage_action_ranks_first() {
        let mut palette = Palette::new(entries(&Keymap::default(), &[]));
        palette.query = "usage".into();
        palette.filter();
        assert_eq!(
            palette.current().unwrap().run,
            Run::Ui(Ui::Settings(Some(crate::settings::Tab::Usage)))
        );
        palette.step(true);
        palette.step(false);
        assert_eq!(palette.selected, 0);
    }
}
