//! `/language [value]` and the Config tab's language row (`localization` → Language
//! selection): a picker over `auto` and the supported languages. A choice is saved as
//! the user's global `language` (`PUT /config/language`), the interface switches at
//! once, and the model replies in it from its next Turn.

use crossterm::event::{KeyCode, KeyEvent};
use serde_json::{Value, json};

use cyber_client::Client;
use cyber_core::locale::Locale;

use crate::app::{Action, App, Overlay};
use crate::output_styles::Opener;
use crate::perform::{Msg, err};

/// Each choice with the name it is shown by: languages in their own language, so
/// anyone can find theirs; `auto` in the interface language.
pub const CHOICES: [(&str, &str); 5] = [
    ("auto", "Automatic (system language)"),
    ("en", "English"),
    ("pt-BR", "Português (Brasil)"),
    ("es", "Español"),
    ("fr", "Français"),
];

/// What the language row and picker ask the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Load,
    Set(String),
}

/// The server's answer: the Location's setting, and the value asked for by a `Set`.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Setting {
        setting: Setting,
        requested: Option<String>,
    },
    Failed(String),
}

/// `GET /config/language`, as the row shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Setting {
    /// `auto` or a tag.
    pub language: String,
    /// The configuration file that set it.
    pub source: Option<String>,
}

impl Setting {
    fn parse(v: &Value) -> Self {
        Self {
            language: v["language"].as_str().unwrap_or("auto").to_owned(),
            source: v["source"].as_str().map(str::to_owned),
        }
    }
}

/// The open picker.
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    /// Index into [`CHOICES`].
    pub selected: usize,
    pub opener: Opener,
}

/// The display name of a choice, translated for `auto`.
pub fn name(app: &App, value: &str) -> String {
    CHOICES.iter().find(|(tag, _)| *tag == value).map_or_else(
        || value.to_owned(),
        |&(tag, name)| {
            if tag == "auto" {
                app.tr(name).to_owned()
            } else {
                name.to_owned()
            }
        },
    )
}

/// The interface language for a setting value: its own, or the environment's for `auto`.
fn interface(value: &str, env: &dyn cyber_core::env::EnvSource) -> Locale {
    match value {
        "auto" => Locale::from_env(env),
        tag => Locale::parse(tag).unwrap_or_default(),
    }
}

impl App {
    /// The language value the row and picker show: the server's, else what the
    /// interface uses.
    pub(crate) fn language_value(&self) -> String {
        self.language
            .as_ref()
            .map_or_else(|| self.locale.tag().to_owned(), |s| s.language.clone())
    }

    /// `/language` opens the picker; `/language <value>` sets it at once.
    pub(crate) fn language_command(&mut self, args: &str) -> Vec<Action> {
        if args.is_empty() {
            return self.open_language(Opener::Command);
        }
        match cyber_core::locale::canonical(args) {
            Some(value) => vec![Action::Language(Request::Set(value))],
            None => {
                let message = self.trf(
                    "language: expected auto, en, pt-BR, es or fr, got {value}",
                    &[("value", &args)],
                );
                self.toast(message);
                Vec::new()
            }
        }
    }

    pub(crate) fn open_language(&mut self, opener: Opener) -> Vec<Action> {
        self.completion = None;
        let current = self.language_value();
        let selected = CHOICES
            .iter()
            .position(|(tag, _)| *tag == current)
            .unwrap_or(0);
        self.overlay = Overlay::Language(Picker { selected, opener });
        vec![Action::Language(Request::Load)]
    }

    fn close_language(&mut self, opener: Opener) {
        self.overlay = match opener {
            Opener::Settings => Overlay::Settings,
            Opener::Command => Overlay::None,
        };
    }

    /// Keys of the picker: Up/Down select, Enter applies, Esc closes.
    pub(crate) fn language_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Overlay::Language(picker) = &mut self.overlay else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => {
                let opener = picker.opener;
                self.close_language(opener);
            }
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => picker.selected = (picker.selected + 1).min(CHOICES.len() - 1),
            KeyCode::Enter => {
                let (opener, value) = (picker.opener, CHOICES[picker.selected].0);
                self.close_language(opener);
                return vec![Action::Language(Request::Set(value.to_owned()))];
            }
            _ => {}
        }
        Vec::new()
    }

    pub(crate) fn language_reply(&mut self, reply: Reply) {
        self.language_applied(reply, &cyber_core::env::ProcessEnv);
    }

    /// Apply a reply: a set language switches the interface at once and is announced;
    /// a project that keeps its own language here is named.
    pub(crate) fn language_applied(&mut self, reply: Reply, env: &dyn cyber_core::env::EnvSource) {
        let (setting, requested) = match reply {
            Reply::Setting { setting, requested } => (setting, requested),
            Reply::Failed(message) => return self.toast(message),
        };
        let Some(requested) = requested else {
            // An open picker moves to the loaded setting, which it marks `current`.
            if let Overlay::Language(picker) = &mut self.overlay
                && let Some(index) = CHOICES.iter().position(|(tag, _)| *tag == setting.language)
            {
                picker.selected = index;
            }
            self.language = Some(setting);
            return;
        };
        self.locale = interface(&setting.language, env);
        let shown = name(self, &setting.language);
        let announced = self.trf("Language: {name}", &[("name", &shown)]);
        self.announcement = Some((announced, self.now()));
        if setting.language != requested {
            let source = setting.source.clone().unwrap_or_default();
            let message = self.trf(
                "{source} sets the language here; your default applies elsewhere",
                &[("source", &source)],
            );
            self.toast(message);
        }
        self.language = Some(setting);
    }
}

pub async fn perform(client: &Client, request: Request) -> Result<Msg, String> {
    let (result, requested) = match request {
        Request::Load => (client.get("/config/language").await, None),
        Request::Set(value) => {
            let body = json!({ "language": value });
            let result = client.request("PUT", "/config/language", Some(body)).await;
            (result, Some(value))
        }
    };
    let reply = match result {
        Ok(body) => Reply::Setting {
            setting: Setting::parse(&body["data"]),
            requested,
        },
        Err(e) => Reply::Failed(format!("language: {}", err(e))),
    };
    Ok(Msg::Language(reply))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env(&'static str);
    impl cyber_core::env::EnvSource for Env {
        fn get(&self, key: &str) -> Option<String> {
            (key == "LANG").then(|| self.0.to_owned())
        }
    }

    #[test]
    fn auto_follows_the_environment_and_a_tag_is_itself() {
        assert_eq!(interface("auto", &Env("fr_FR.UTF-8")), Locale::Fr);
        assert_eq!(interface("pt-BR", &Env("fr_FR.UTF-8")), Locale::PtBr);
    }

    #[test]
    fn choices_are_the_supported_values() {
        let tags: Vec<String> = CHOICES.iter().map(|(t, _)| (*t).to_owned()).collect();
        assert_eq!(tags, cyber_core::locale::choices());
    }
}
