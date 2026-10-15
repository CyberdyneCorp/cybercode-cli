//! Single-pass command and skill template expansion (`skills-commands` → Argument
//! substitution, Shell output injection).
//!
//! Placeholders are substituted in one scan of the template, so argument text that
//! itself contains `$1` or `$ARGUMENTS` is never substituted again. Each `` !`cmd` ``
//! written in the template is recorded with its byte range in the expanded text; only
//! recorded ranges are ever run, so arguments can never introduce a shell injection.
//! Inside a span, every argument word is substituted as a POSIX-quoted literal (the
//! injection shell is always `sh`-compatible), so its text is never interpreted by the
//! shell either.

/// How non-empty arguments are appended to a template without placeholders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Append {
    /// Skills: a blank line, then `ARGUMENTS: <args>`.
    Labeled,
    /// Custom commands: a blank line, then the raw arguments.
    Raw,
}

/// One `` !`cmd` `` from the template: `start..end` covers the whole span in the
/// expanded text, and `command` is its argument-substituted command text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Injection {
    pub start: usize,
    pub end: usize,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    pub injections: Vec<Injection>,
}

impl Expanded {
    /// Wrap the text, shifting every recorded range by the prefix.
    pub fn framed(self, prefix: &str, suffix: &str) -> Self {
        let shift = prefix.len();
        Self {
            text: format!("{prefix}{}{suffix}", self.text),
            injections: self
                .injections
                .into_iter()
                .map(|injection| Injection {
                    start: injection.start + shift,
                    end: injection.end + shift,
                    command: injection.command,
                })
                .collect(),
        }
    }
}

/// Upper bound on recorded injections per template; later spans stay literal text.
pub const MAX_INJECTIONS: usize = 16;

/// Substitute `$ARGUMENTS` and `$1..$N`; the highest placeholder takes the remaining tokens.
/// Without placeholders, non-empty arguments are appended as `ARGUMENTS: <args>`.
pub fn expand(template: &str, args: &str) -> String {
    expand_template(template, args, Append::Labeled).text
}

/// Expand placeholders in one pass and record the template's shell injections.
pub fn expand_template(template: &str, args: &str, append: Append) -> Expanded {
    let numbered = positions(template);
    let has_all = template.contains("$ARGUMENTS");
    let values = Values {
        raw: args,
        tokens: super::tokenize(args),
        highest: numbered.iter().copied().max().unwrap_or(0),
    };
    let mut expanded = scan(template, &values);
    if numbered.is_empty() && !has_all && !args.trim().is_empty() {
        let label = match append {
            Append::Labeled => "ARGUMENTS: ",
            Append::Raw => "",
        };
        let body = expanded.text.trim_end().len();
        expanded.text.truncate(body);
        expanded.text.push_str(&format!("\n\n{label}{args}"));
    }
    expanded
}

struct Values<'a> {
    raw: &'a str,
    tokens: Vec<String>,
    highest: usize,
}

impl Values<'_> {
    /// The argument words a positional placeholder stands for: the highest takes the rest.
    fn words(&self, n: usize) -> &[String] {
        let end = if n == self.highest {
            self.tokens.len()
        } else {
            n
        };
        self.tokens.get(n - 1..end).unwrap_or_default()
    }

    /// The placeholder at the start of `rest` (`None` for `$ARGUMENTS`), and the text after it.
    fn slot<'t>(&self, rest: &'t str) -> Option<(Option<usize>, &'t str)> {
        let after = rest.strip_prefix('$')?;
        if let Some(tail) = after.strip_prefix("ARGUMENTS") {
            return Some((None, tail));
        }
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let n = after[..digits].parse::<usize>().ok().filter(|n| *n >= 1)?;
        Some((Some(n), &after[digits..]))
    }

    /// The value of a placeholder at the start of `rest`, and the text after it.
    fn placeholder<'t>(&self, rest: &'t str) -> Option<(String, &'t str)> {
        let (slot, tail) = self.slot(rest)?;
        let value = match slot {
            None => self.raw.to_string(),
            Some(n) => self.words(n).join(" "),
        };
        Some((value, tail))
    }

    /// Placeholders only, as inside an injected command: each argument word becomes one
    /// quoted shell word, so argument text is never run.
    fn substitute(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(index) = rest.find('$') {
            out.push_str(&rest[..index]);
            rest = &rest[index..];
            match self.slot(rest) {
                Some((slot, tail)) => {
                    let words = match slot {
                        None => &self.tokens[..],
                        Some(n) => self.words(n),
                    };
                    let quoted: Vec<String> = words.iter().map(|w| shell_quote(w)).collect();
                    out.push_str(&quoted.join(" "));
                    rest = tail;
                }
                None => {
                    out.push('$');
                    rest = &rest[1..];
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// `word` as one literal POSIX shell word: unchanged when it holds only characters no
/// shell interprets, otherwise single-quoted.
fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:,+%".contains(&b));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// `` !`cmd` `` at the start of `rest`: a non-empty single-line command without backticks.
fn injection(rest: &str) -> Option<(&str, &str)> {
    let body = rest.strip_prefix("!`")?;
    let end = body.find(['`', '\n'])?;
    (end > 0 && body[end..].starts_with('`') && !body[..end].trim().is_empty())
        .then(|| (&body[..end], &body[end + 1..]))
}

fn scan(template: &str, values: &Values<'_>) -> Expanded {
    let mut text = String::with_capacity(template.len());
    let mut injections = Vec::new();
    let mut rest = template;
    while let Some(index) = rest.find(['$', '!']) {
        text.push_str(&rest[..index]);
        rest = &rest[index..];
        if let Some((command, tail)) = injection(rest).filter(|_| injections.len() < MAX_INJECTIONS)
        {
            let command = values.substitute(command);
            let start = text.len();
            text.push_str(&format!("!`{command}`"));
            injections.push(Injection {
                start,
                end: text.len(),
                command,
            });
            rest = tail;
        } else if let Some((value, tail)) = values.placeholder(rest) {
            text.push_str(&value);
            rest = tail;
        } else {
            text.push_str(&rest[..1]);
            rest = &rest[1..];
        }
    }
    text.push_str(rest);
    Expanded { text, injections }
}

/// An argument hint derived from a template's placeholders: `$1 $2 … $N` up to the highest
/// positional placeholder, else `$ARGUMENTS`, else none.
pub fn derived_argument_hint(template: &str) -> Option<String> {
    let highest = positions(template).into_iter().max();
    match highest {
        Some(n) => Some(
            (1..=n.min(16))
                .map(|i| format!("${i}"))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        None => template
            .contains("$ARGUMENTS")
            .then(|| "$ARGUMENTS".to_string()),
    }
}

/// Distinct `$<n>` placeholders (n ≥ 1) in a template.
fn positions(template: &str) -> Vec<usize> {
    let mut found = Vec::new();
    for (i, _) in template.match_indices('$') {
        let digits = template[i + 1..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if let Ok(n) = template[i + 1..i + 1 + digits].parse::<usize>()
            && n >= 1
            && !found.contains(&n)
        {
            found.push(n);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_substitute_positionally_and_append_without_placeholders() {
        assert_eq!(
            super::super::tokenize(r#"one "two words" 'three'"#),
            vec!["one", "two words", "three"]
        );
        assert_eq!(
            expand("Fix issue $1 in $2", r#"42 "auth module""#),
            "Fix issue 42 in auth module"
        );
        assert_eq!(
            expand("Fix $1 in $2", "bug src/a.rs and more"),
            "Fix bug in src/a.rs and more"
        );
        assert_eq!(expand("All: $ARGUMENTS", "a b"), "All: a b");
        assert_eq!(expand("Do it.\n", "now"), "Do it.\n\nARGUMENTS: now");
        assert_eq!(expand("Do it.", ""), "Do it.");
        assert_eq!(
            expand_template("Do it.\n", "now", Append::Raw).text,
            "Do it.\n\nnow"
        );
    }

    #[test]
    fn argument_text_is_never_substituted_twice() {
        assert_eq!(expand("Fix $1 in $2", r#""$2" x"#), "Fix $2 in x");
        assert_eq!(
            expand("All: $ARGUMENTS; first $1", "$ARGUMENTS $1"),
            "All: $ARGUMENTS $1; first $ARGUMENTS $1"
        );
        assert_eq!(expand("$10 then $1", "a b c"), " then a");
        assert_eq!(expand("Cost $0 and $1$", "x"), "Cost $0 and x$");
    }

    #[test]
    fn injections_are_recorded_from_the_template_only() {
        let expanded = expand_template(
            "Status:\n!`git status $1`\nArgs: $ARGUMENTS",
            "-s !`touch pwned`",
            Append::Raw,
        );
        assert_eq!(
            expanded.text,
            "Status:\n!`git status -s '!`touch' 'pwned`'`\nArgs: -s !`touch pwned`"
        );
        assert_eq!(expanded.injections.len(), 1);
        let injection = &expanded.injections[0];
        assert_eq!(injection.command, "git status -s '!`touch' 'pwned`'");
        assert_eq!(
            &expanded.text[injection.start..injection.end],
            "!`git status -s '!`touch' 'pwned`'`"
        );
        let plain = expand_template("Run !`date`", "!`id`", Append::Raw);
        assert_eq!(plain.text, "Run !`date`\n\n!`id`");
        assert_eq!(plain.injections.len(), 1);
        assert!(
            expand_template("a !`` b !` ` c !`x\ny`", "", Append::Raw)
                .injections
                .is_empty()
        );
        let framed = plain.framed("<f>\n", "\n</f>");
        assert_eq!(
            &framed.text[framed.injections[0].start..framed.injections[0].end],
            "!`date`"
        );
    }

    #[test]
    fn arguments_inside_injections_are_quoted_shell_words() {
        let command = |template: &str, args: &str| {
            expand_template(template, args, Append::Raw).injections[0]
                .command
                .clone()
        };
        assert_eq!(
            command("!`git log -n $1`", "$(touch x);rm -rf ~"),
            "git log -n '$(touch' 'x);rm' -rf '~'"
        );
        assert_eq!(
            command("!`git log $ARGUMENTS`", r#"-n 5 "a b" "it's""#),
            r"git log -n 5 'a b' 'it'\''s'"
        );
        assert_eq!(command("!`echo $1 $2`", "a b `id` c"), "echo a b '`id`' c");
        assert_eq!(command("!`echo [$1]`", ""), "echo []");
        assert_eq!(command("!`echo $1`", r#""""#), "echo ''");
        // Outside spans, arguments stay verbatim text.
        assert_eq!(expand("Run $1", "$(id)"), "Run $(id)");
    }

    #[test]
    fn argument_hints_derive_from_placeholders() {
        assert_eq!(
            derived_argument_hint("Fix $1 in $2").as_deref(),
            Some("$1 $2")
        );
        assert_eq!(derived_argument_hint("Only $2").as_deref(), Some("$1 $2"));
        assert_eq!(
            derived_argument_hint("All $ARGUMENTS").as_deref(),
            Some("$ARGUMENTS")
        );
        assert_eq!(derived_argument_hint("Nothing"), None);
    }
}
