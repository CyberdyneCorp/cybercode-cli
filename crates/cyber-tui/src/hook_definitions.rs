//! Review redacted definitions before changing exact checkout-scoped approvals.
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
struct Definition {
    event: String,
    scope: String,
    source: String,
    pointer: String,
    matcher: Option<String>,
    paths: Vec<String>,
    handler: Value,
    digest: String,
    trusted: bool,
    sandbox_required: bool,
}
impl Definition {
    fn checkout_scoped(&self) -> bool {
        matches!(self.scope.as_str(), "project" | "local")
    }
}
#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    hooks: Vec<Definition>,
    withheld_definitions: Vec<String>,
    checkout_trusted: bool,
}
impl Catalog {
    pub fn parse(value: &Value) -> Result<Self, String> {
        let mut catalog: Self = Self::deserialize(&value["data"])
            .map_err(|_| "Malformed hook definition catalog".to_string())?;
        for row in &mut catalog.hooks {
            if !row
                .digest
                .strip_prefix("sha256:")
                .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                || !matches!(
                    row.scope.as_str(),
                    "managed" | "global" | "project" | "local" | "plugin" | "invocation"
                )
                || !row.handler.is_object()
            {
                return Err("Invalid hook definition metadata".into());
            }
            row.handler = cyber_core::config::redact_secrets(&row.handler);
        }
        Ok(catalog)
    }
}
#[derive(Debug, Default)]
pub struct View {
    generation: u64,
    pending: bool,
    catalog: Option<Catalog>,
    error: Option<String>,
    confirmation: Option<bool>,
    selected: usize,
    pub scroll: u16,
}
impl View {
    pub fn load(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.pending = true;
        self.catalog = None;
        self.error = None;
        self.confirmation = None;
        self.scroll = 0;
        self.generation
    }
    pub fn invalidate(&mut self) {
        self.load();
        self.pending = false;
    }
    pub fn apply(&mut self, generation: u64, result: Result<Catalog, String>) {
        if generation != self.generation || !self.pending {
            return;
        }
        self.pending = false;
        match result {
            Ok(catalog) => {
                self.selected = self.selected.min(catalog.hooks.len().saturating_sub(1));
                self.catalog = Some(catalog);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn select(&mut self, next: bool) {
        if self.pending || self.confirmation.is_some() {
            return;
        }
        let len = self.catalog.as_ref().map_or(0, |c| c.hooks.len());
        self.selected = if next {
            self.selected.saturating_add(1).min(len.saturating_sub(1))
        } else {
            self.selected.saturating_sub(1)
        };
        self.scroll = 0;
    }
    pub fn confirm(&mut self, approve: bool) {
        if self.pending {
            return;
        }
        let Some(catalog) = &self.catalog else {
            return;
        };
        let Some(row) = catalog.hooks.get(self.selected) else {
            return;
        };
        if row.checkout_scoped() && row.trusted != approve && (!approve || catalog.checkout_trusted)
        {
            self.confirmation = Some(approve);
        }
    }
    pub fn cancel_confirmation(&mut self) -> bool {
        self.confirmation.take().is_some()
    }
    pub fn change(&mut self) -> Option<(u64, String, bool)> {
        let approve = self.confirmation.take()?;
        let digest = self
            .catalog
            .as_ref()?
            .hooks
            .get(self.selected)?
            .digest
            .clone();
        self.generation = self.generation.wrapping_add(1);
        self.pending = true;
        self.error = None;
        Some((self.generation, digest, approve))
    }
    pub fn lines(&self) -> Vec<String> {
        let mut lines =
            vec!["Hook definitions · ↑/↓ select · PgUp/PgDn scroll · R refresh · Esc close".into()];
        if let Some(error) = &self.error {
            lines.push(format!("Request failed: {}", safe(error)));
        }
        if self.pending {
            lines.push("Loading current hook definitions/trust…".into());
        }
        let Some(catalog) = &self.catalog else {
            return lines;
        };
        lines.push(format!(
            "Checkout trusted: {} · {} definitions",
            catalog.checkout_trusted,
            catalog.hooks.len()
        ));
        for path in &catalog.withheld_definitions {
            lines.push(format!(
                "Withheld: {} (review checkout configuration with cyber trust)",
                safe(path)
            ));
        }
        let Some(row) = catalog.hooks.get(self.selected) else {
            lines.push("No loaded hook definitions.".into());
            return lines;
        };
        lines.extend([
            format!(
                "Definition {}/{} · {} · {}",
                self.selected + 1,
                catalog.hooks.len(),
                safe(&row.event),
                safe(&row.scope)
            ),
            format!(
                "Source: {} · pointer: {}",
                safe(&row.source),
                safe(&row.pointer)
            ),
            format!("Digest: {}", safe(&row.digest)),
            format!(
                "Trusted: {} · sandbox required: {}",
                row.trusted, row.sandbox_required
            ),
            format!(
                "Matcher: {} · paths: {}",
                safe(row.matcher.as_deref().unwrap_or("*")),
                safe(&row.paths.join(", "))
            ),
            "Handler (credentials redacted):".into(),
        ]);
        lines.extend(
            serde_json::to_string_pretty(&row.handler)
                .unwrap_or_default()
                .lines()
                .map(safe),
        );
        if let Some(approve) = self.confirmation {
            lines.push(format!(
                "{} this exact digest? Y confirm · N/Esc cancel",
                if approve { "Approve" } else { "Revoke" }
            ));
        } else if row.checkout_scoped() {
            lines.push("T review approval · U review revocation (checkout-scoped)".into());
        } else {
            lines.push("This scope does not use individual checkout approvals.".into());
        }
        lines
    }
}
fn safe(value: &str) -> String {
    value.escape_debug().to_string()
}
