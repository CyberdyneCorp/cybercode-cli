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

pub(super) fn parse(words: &[String]) -> Invocation<'_> {
    let mut invocation = Invocation {
        no_profile: false,
        startup_certain: true,
        source: Source::Stdin,
    };
    let mut index = 0;
    let mut encoded = false;
    while let Some(arg) = words.get(index) {
        match arg.to_ascii_lowercase().as_str() {
            "-noprofile" | "-nop" => invocation.no_profile = true,
            "-nologo" | "-nol" | "-noninteractive" | "-noni" => {}
            "-command" | "-c" => {
                invocation.startup_certain &= !encoded;
                invocation.source = if words.get(index + 1).is_some_and(|v| v == "-") {
                    invocation.startup_certain &= index + 2 == words.len();
                    Source::Stdin
                } else {
                    Source::Text(&words[index + 1..])
                };
                return invocation;
            }
            "-encodedcommand" | "-e" | "-ec" | "-enc" => {
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
            "-file" | "-f" => {
                invocation.startup_certain &= !encoded;
                invocation.source = if words.get(index + 1).is_some_and(|v| v == "-") {
                    invocation.startup_certain &= index + 2 == words.len();
                    Source::Stdin
                } else {
                    Source::Missing
                };
                return invocation;
            }
            "-help" | "-?" | "/?" if index + 1 == words.len() && !encoded => {
                invocation.source = Source::Help;
                return invocation;
            }
            "-workingdirectory" | "-wd" | "-wo" | "-executionpolicy" | "-ex" | "-ep"
            | "-configurationname" | "-config" | "-configurationfile" | "-custompipename"
            | "-inputformat" | "-inp" | "-if" | "-outputformat" | "-o" | "-of"
            | "-settingsfile" | "-settings" | "-psconsolefile" => {
                invocation.startup_certain = false;
                index += 1;
            }
            value if !value.starts_with('-') => {
                invocation.startup_certain &= !encoded;
                invocation.source = Source::Missing;
                return invocation;
            }
            _ => invocation.startup_certain = false,
        }
        index += 1;
    }
    invocation
}
