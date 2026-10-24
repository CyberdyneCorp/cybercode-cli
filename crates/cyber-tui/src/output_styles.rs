//! `/output-style [name]` (`tui` → Output style picker): the styles the server lists for
//! the Session's Location (`GET /config/output-styles`), with their origin and the
//! `current` and `default` labels and the listing's issues. Enter switches the Session,
//! `d` also saves the style as the user's default, and Esc closes without side effects.
//! A switch applies from the next Turn, which the footer announces.

use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use cyber_client::Client;

use crate::app::{Action, App, Overlay};
use crate::perform::{Msg, err};

/// What a subagent thread says to `/output-style`.
pub const SUBAGENT: &str = "Output styles apply to primary Sessions, not to subagent threads";

/// What the picker asks the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Read the listing; `open` opens the picker on it.
    Load {
        session_id: String,
        open: Option<Opener>,
    },
    /// Switch the Session; `save` first makes `style` the user's global default.
    Switch {
        session_id: String,
        style: String,
        save: bool,
    },
}

/// Where the picker was opened from, which Esc and Enter return to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opener {
    Command,
    Settings,
}

/// The server's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Listing {
        session_id: String,
        open: Option<Opener>,
        listing: Listing,
    },
    Switched {
        session_id: String,
        style: String,
        saved: bool,
    },
    Failed(String),
}

/// One listed style.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub name: String,
    pub description: String,
    /// `built-in`, `user` or `project`.
    pub origin: String,
}

/// `GET /config/output-styles`, as the picker shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    pub styles: Vec<Style>,
    /// `path: message` of each issue, such as held project styles.
    pub issues: Vec<String>,
    /// The configured `output_style` and the file that set it.
    pub configured: Option<(String, String)>,
    /// The style a new Session uses.
    pub effective: String,
    /// Why the configured style does not apply.
    pub unavailable: Option<String>,
}

impl Listing {
    pub fn parse(v: &Value) -> Self {
        let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
        let styles = v["styles"].as_array().into_iter().flatten();
        let issues = v["issues"].as_array().into_iter().flatten();
        Self {
            styles: styles
                .map(|s| Style {
                    name: text(&s["name"]),
                    description: text(&s["description"]),
                    origin: match s["origin"].as_str() {
                        Some("builtin") | None => "built-in".into(),
                        Some(origin) => origin.into(),
                    },
                })
                .collect(),
            issues: issues
                .map(|i| format!("{}: {}", text(&i["path"]), text(&i["message"])))
                .collect(),
            configured: v["configured"]["name"]
                .as_str()
                .map(|name| (name.to_owned(), text(&v["configured"]["source"]))),
            effective: text(&v["effective"]),
            unavailable: v["unavailable"].as_str().map(str::to_owned),
        }
    }
}

/// The loaded listing and the Sessions already told that their style is unavailable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutputStyles {
    pub listing: Option<Listing>,
    noticed: BTreeSet<String>,
}

/// The open picker.
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub query: String,
    /// Index into [`Picker::visible`].
    pub selected: usize,
    pub opener: Opener,
}

impl Picker {
    /// Indices of the styles the search admits, best match first.
    pub fn visible(&self, listing: &Listing) -> Vec<usize> {
        crate::fuzzy::rank(&self.query, &listing.styles, |s| s.name.clone())
    }

    fn current<'a>(&self, listing: &'a Listing) -> Option<&'a Style> {
        let visible = self.visible(listing);
        visible.get(self.selected).map(|&i| &listing.styles[i])
    }
}

impl App {
    /// Whether this thread may have an output style: subagents and teammates have none.
    fn takes_output_style(&self) -> bool {
        self.session.parent_id.is_none() && self.session.team.is_none()
    }

    /// The Session's effective style and its source: `session`, `config` or `default`.
    pub fn output_style(&self) -> (String, &'static str) {
        if let Some(own) = &self.session.output_style {
            return (own.clone(), "session");
        }
        match self.output_styles.listing.as_ref() {
            Some(listing) if listing.configured.is_some() => (listing.effective.clone(), "config"),
            Some(_) => (cyber_core::output_styles::DEFAULT.into(), "default"),
            None => (String::new(), ""),
        }
    }

    /// `/output-style` opens the picker, `/output-style <name>` switches at once.
    pub(crate) fn output_style_command(&mut self, args: &str) -> Vec<Action> {
        if !self.takes_output_style() {
            self.toast(SUBAGENT);
            return Vec::new();
        }
        let session_id = self.session.id.clone();
        if args.is_empty() {
            return self.open_output_styles(Opener::Command);
        }
        vec![Action::OutputStyle(Request::Switch {
            session_id,
            style: args.to_owned(),
            save: false,
        })]
    }

    /// Load the listing and open the picker on it.
    pub(crate) fn open_output_styles(&mut self, opener: Opener) -> Vec<Action> {
        self.completion = None;
        vec![Action::OutputStyle(Request::Load {
            session_id: self.session.id.clone(),
            open: Some(opener),
        })]
    }

    /// A load of the listing for the unavailable-style notice and the Config tab.
    pub(crate) fn output_style_check(&self) -> Vec<Action> {
        if self.session.id.is_empty() || !self.takes_output_style() {
            return Vec::new();
        }
        vec![Action::OutputStyle(Request::Load {
            session_id: self.session.id.clone(),
            open: None,
        })]
    }

    pub(crate) fn output_style_reply(&mut self, reply: Reply) -> Vec<Action> {
        match reply {
            Reply::Listing {
                session_id,
                open,
                listing,
            } if session_id == self.session.id => {
                self.notice_unavailable(&listing);
                self.output_styles.listing = Some(listing);
                if let Some(opener) = open {
                    self.overlay = Overlay::OutputStyles(Picker {
                        query: String::new(),
                        selected: 0,
                        opener,
                    });
                    self.select_current_style();
                }
            }
            Reply::Switched {
                session_id,
                style,
                saved,
            } if session_id == self.session.id => {
                self.session.output_style = Some(style.clone());
                self.announcement =
                    Some((format!("Output style: {style} (next turn)"), self.now()));
                if saved {
                    self.toast(format!("Saved {style} as the default output style"));
                }
                return self.output_style_check();
            }
            Reply::Failed(message) => self.toast(message),
            _ => {}
        }
        Vec::new()
    }

    /// The reason the Session's style does not apply, once per Session.
    fn notice_unavailable(&mut self, listing: &Listing) {
        let reason = match &self.session.output_style {
            Some(own) if !listing.styles.iter().any(|s| &s.name == own) => {
                Some(format!("output style \"{own}\" is not available here"))
            }
            Some(_) => None,
            None => listing.unavailable.clone(),
        };
        if let Some(reason) = reason
            && self.output_styles.noticed.insert(self.session.id.clone())
        {
            self.toast(format!("{reason}; the default output style applies"));
        }
    }

    fn select_current_style(&mut self) {
        let (current, _) = self.output_style();
        let (Overlay::OutputStyles(picker), Some(listing)) =
            (&mut self.overlay, &self.output_styles.listing)
        else {
            return;
        };
        let visible = picker.visible(listing);
        picker.selected = visible
            .iter()
            .position(|&i| listing.styles[i].name == current)
            .unwrap_or(0);
    }

    fn close_output_styles(&mut self, opener: Opener) {
        self.overlay = match opener {
            Opener::Settings => Overlay::Settings,
            Opener::Command => Overlay::None,
        };
    }

    /// Keys of the picker: type to filter, Enter switches, `d` saves the default too.
    pub(crate) fn output_styles_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let (Overlay::OutputStyles(picker), Some(listing)) =
            (&mut self.overlay, &self.output_styles.listing)
        else {
            return Vec::new();
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                let opener = picker.opener;
                self.close_output_styles(opener);
            }
            KeyCode::Enter => return self.pick_output_style(false),
            KeyCode::Char('d') if picker.query.is_empty() && !ctrl => {
                return self.pick_output_style(true);
            }
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => {
                let last = picker.visible(listing).len().saturating_sub(1);
                picker.selected = (picker.selected + 1).min(last);
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.selected = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                picker.query.push(c);
                picker.selected = 0;
            }
            _ => {}
        }
        Vec::new()
    }

    fn pick_output_style(&mut self, save: bool) -> Vec<Action> {
        let (Overlay::OutputStyles(picker), Some(listing)) =
            (&self.overlay, &self.output_styles.listing)
        else {
            return Vec::new();
        };
        let opener = picker.opener;
        let Some(style) = picker.current(listing).map(|s| s.name.clone()) else {
            return Vec::new();
        };
        self.close_output_styles(opener);
        vec![Action::OutputStyle(Request::Switch {
            session_id: self.session.id.clone(),
            style,
            save,
        })]
    }
}

pub async fn perform(client: &Client, request: Request) -> Result<Msg, String> {
    let reply = match request {
        Request::Load { session_id, open } => match client.get("/config/output-styles").await {
            Ok(body) => Reply::Listing {
                session_id,
                open,
                listing: Listing::parse(&body["data"]),
            },
            Err(e) => Reply::Failed(format!("output styles: {}", err(e))),
        },
        Request::Switch {
            session_id,
            style,
            save,
        } => match switch(client, &session_id, &style, save).await {
            Ok(()) => Reply::Switched {
                session_id,
                style,
                saved: save,
            },
            Err(message) => Reply::Failed(message),
        },
    };
    Ok(Msg::OutputStyle(reply))
}

async fn switch(client: &Client, session_id: &str, style: &str, save: bool) -> Result<(), String> {
    let body = json!({ "style": style });
    if save {
        client
            .request("PUT", "/config/output-style", Some(body.clone()))
            .await
            .map_err(|e| format!("default output style: {}", err(e)))?;
    }
    client
        .post(&format!("/sessions/{session_id}/output-style"), body)
        .await
        .map_err(|e| format!("output style: {}", err(e)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_parses_origins_issues_and_the_configured_style() {
        let listing = Listing::parse(&json!({
            "styles": [
                {"name": "default", "description": "", "origin": "builtin"},
                {"name": "terse", "description": "Be terse", "origin": "user"},
            ],
            "issues": [{"path": "/r/.cyber/output-styles", "message": "held"}],
            "configured": {"name": "ghost", "source": "/u/cyber.jsonc"},
            "effective": "default",
            "unavailable": "unknown output style \"ghost\"",
        }));
        assert_eq!(listing.styles[0].origin, "built-in");
        assert_eq!(listing.styles[1].origin, "user");
        assert_eq!(listing.issues, ["/r/.cyber/output-styles: held"]);
        assert_eq!(
            listing.configured,
            Some(("ghost".into(), "/u/cyber.jsonc".into()))
        );
        assert_eq!(listing.effective, "default");
        assert!(listing.unavailable.is_some());
    }
}
