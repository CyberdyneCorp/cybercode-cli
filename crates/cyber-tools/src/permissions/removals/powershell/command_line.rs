//! Parse source selection without confusing script arguments with startup options.

pub(super) enum Source<'a> {
    Text(&'a [String]),
    Encoded(&'a str),
    Stdin,
    Help,
    Missing,
}

pub(super) struct Invocation<'a> {
    pub no_profile: bool,
    pub startup_certain: bool,
    pub source: Source<'a>,
}

#[derive(Clone, Copy)]
enum Switch {
    NoProfile,
    Quiet,
    Command,
    Encoded,
    File,
    Help,
    /// An option with a value that may change how the source runs.
    Valued,
}

/// Startup switches as `(name, shortest accepted prefix, meaning)`, in the order the
/// interpreter matches them: any prefix of `name` at least as long as the second entry
/// selects it. `-co` stays unknown rather than guessing between Command and Configuration.
const SWITCHES: &[(&str, &str, Switch)] = &[
    ("c", "c", Switch::Command),
    ("command", "com", Switch::Command),
    ("e", "e", Switch::Encoded),
    ("ec", "ec", Switch::Encoded),
    ("encodedcommand", "en", Switch::Encoded),
    ("encodedarguments", "encodeda", Switch::Valued),
    ("ea", "ea", Switch::Valued),
    ("f", "f", Switch::File),
    ("file", "fi", Switch::File),
    ("noprofile", "nop", Switch::NoProfile),
    ("nologo", "nol", Switch::Quiet),
    ("noninteractive", "noni", Switch::Quiet),
    ("help", "help", Switch::Help),
    ("?", "?", Switch::Help),
    ("workingdirectory", "wo", Switch::Valued),
    ("wd", "wd", Switch::Valued),
    ("executionpolicy", "ex", Switch::Valued),
    ("ep", "ep", Switch::Valued),
    ("configurationname", "config", Switch::Valued),
    ("configurationfile", "configurationf", Switch::Valued),
    ("custompipename", "custompipename", Switch::Valued),
    ("inputformat", "inp", Switch::Valued),
    ("if", "if", Switch::Valued),
    ("outputformat", "o", Switch::Valued),
    ("of", "of", Switch::Valued),
    ("settingsfile", "settings", Switch::Valued),
    ("psconsolefile", "psconsolefile", Switch::Valued),
];

fn switch(arg: &str) -> Option<Switch> {
    if arg == "/?" {
        return Some(Switch::Help);
    }
    let key = arg.strip_prefix('-')?.to_ascii_lowercase();
    SWITCHES
        .iter()
        .find(|(name, shortest, _)| name.starts_with(&key) && key.len() >= shortest.len())
        .map(|(_, _, meaning)| *meaning)
}

pub(super) fn parse(words: &[String]) -> Invocation<'_> {
    let mut invocation = Invocation {
        no_profile: false,
        startup_certain: true,
        source: Source::Stdin,
    };
    let mut index = 0;
    let mut encoded = false;
    while let Some(arg) = words.get(index) {
        match switch(arg) {
            Some(Switch::NoProfile) => invocation.no_profile = true,
            Some(Switch::Quiet) => {}
            Some(Switch::Command) => {
                invocation.startup_certain &= !encoded;
                invocation.source = if words.get(index + 1).is_some_and(|v| v == "-") {
                    invocation.startup_certain &= index + 2 == words.len();
                    Source::Stdin
                } else {
                    Source::Text(&words[index + 1..])
                };
                return invocation;
            }
            Some(Switch::Encoded) => {
                if encoded {
                    invocation.startup_certain = false;
                    invocation.source = Source::Missing;
                    return invocation;
                }
                invocation.source = words
                    .get(index + 1)
                    .map_or(Source::Missing, |v| Source::Encoded(v));
                encoded = true;
                index += 1;
            }
            Some(Switch::File) => {
                invocation.startup_certain &= !encoded;
                invocation.source = if words.get(index + 1).is_some_and(|v| v == "-") {
                    invocation.startup_certain &= index + 2 == words.len();
                    Source::Stdin
                } else {
                    Source::Missing
                };
                return invocation;
            }
            Some(Switch::Help) if index + 1 == words.len() && !encoded => {
                invocation.source = Source::Help;
                return invocation;
            }
            Some(Switch::Valued) => {
                invocation.startup_certain = false;
                index += 1;
            }
            None if !arg.starts_with('-') => {
                invocation.startup_certain &= !encoded;
                invocation.source = Source::Missing;
                return invocation;
            }
            Some(Switch::Help) | None => invocation.startup_certain = false,
        }
        index += 1;
    }
    invocation
}
