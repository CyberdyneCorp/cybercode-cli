//! Cmdlet parameter names as the binder resolves them: an exact name or alias, else the
//! only declared name the text abbreviates.

/// Common parameters every cmdlet accepts, with their aliases.
pub(super) const COMMON: &[&str] = &[
    "verbose",
    "debug",
    "erroraction",
    "warningaction",
    "informationaction",
    "progressaction",
    "errorvariable",
    "warningvariable",
    "informationvariable",
    "outvariable",
    "outbuffer",
    "pipelinevariable",
    "whatif",
    "confirm",
];

pub(super) const COMMON_ALIASES: &[(&str, &str)] = &[
    ("vb", "verbose"),
    ("db", "debug"),
    ("ea", "erroraction"),
    ("wa", "warningaction"),
    ("infa", "informationaction"),
    ("proga", "progressaction"),
    ("ev", "errorvariable"),
    ("wv", "warningvariable"),
    ("iv", "informationvariable"),
    ("ov", "outvariable"),
    ("ob", "outbuffer"),
    ("pv", "pipelinevariable"),
    ("wi", "whatif"),
    ("cf", "confirm"),
    ("lp", "literalpath"),
    ("pspath", "literalpath"),
];

/// The declared parameters (and parameter aliases) of each mutating cmdlet, beyond the
/// common ones. The binder resolves an abbreviation against the cmdlet's own names, so
/// a prefix that names one parameter of another cmdlet (`-U`, `uri` of
/// `Invoke-WebRequest`) can be a switch here (`UseMinimalHeader` of
/// `Start-Transcript`), and the following path then binds positionally. `None` for a
/// command without a list: it resolves against every known name.
pub(super) fn of_cmdlet(name: &str) -> Option<&'static [&'static str]> {
    const CONTENT: &[&str] = &[
        "path",
        "literalpath",
        "value",
        "passthru",
        "filter",
        "include",
        "exclude",
        "force",
        "credential",
        "nonewline",
        "encoding",
        "asbytestream",
        "stream",
    ];
    let names: &[&str] = match name {
        "set-content" | "sc" | "add-content" | "ac" => CONTENT,
        "clear-content" | "clc" => &[
            "path",
            "literalpath",
            "filter",
            "include",
            "exclude",
            "force",
            "credential",
            "stream",
        ],
        "out-file" => &[
            "filepath",
            "path",
            "literalpath",
            "encoding",
            "append",
            "force",
            "noclobber",
            "width",
            "nonewline",
            "inputobject",
        ],
        "tee-object" | "tee" => &[
            "inputobject",
            "filepath",
            "path",
            "literalpath",
            "append",
            "encoding",
            "variable",
        ],
        "new-item" | "ni" => &[
            "path",
            "itemtype",
            "type",
            "name",
            "value",
            "force",
            "credential",
        ],
        "remove-item" | "ri" | "rm" | "rmdir" | "rd" | "del" | "erase" => &[
            "path",
            "literalpath",
            "filter",
            "include",
            "exclude",
            "recurse",
            "force",
            "credential",
            "stream",
        ],
        "export-csv" | "epcsv" => &[
            "inputobject",
            "path",
            "literalpath",
            "force",
            "noclobber",
            "encoding",
            "append",
            "delimiter",
            "useculture",
            "includetypeinformation",
            "iti",
            "notypeinformation",
            "nti",
            "quotefields",
            "qf",
            "usequotes",
            "uq",
            "noheader",
        ],
        "export-clixml" => &[
            "path",
            "literalpath",
            "inputobject",
            "depth",
            "force",
            "noclobber",
            "encoding",
        ],
        "set-item" | "si" => &[
            "path",
            "literalpath",
            "value",
            "force",
            "passthru",
            "filter",
            "include",
            "exclude",
            "credential",
        ],
        "start-transcript" => &[
            "path",
            "literalpath",
            "outputdirectory",
            "append",
            "force",
            "noclobber",
            "includeinvocationheader",
            "useminimalheader",
        ],
        "copy-item" | "cpi" | "cp" | "copy" => &[
            "path",
            "literalpath",
            "destination",
            "container",
            "force",
            "filter",
            "include",
            "exclude",
            "recurse",
            "passthru",
            "credential",
            "fromsession",
            "tosession",
        ],
        "move-item" | "mi" | "mv" | "move" => &[
            "path",
            "literalpath",
            "destination",
            "force",
            "filter",
            "include",
            "exclude",
            "passthru",
            "credential",
        ],
        "rename-item" | "rni" | "ren" => &[
            "path",
            "literalpath",
            "newname",
            "force",
            "passthru",
            "credential",
        ],
        "expand-archive" => &[
            "path",
            "literalpath",
            "destinationpath",
            "force",
            "passthru",
        ],
        "compress-archive" => &[
            "path",
            "literalpath",
            "destinationpath",
            "compressionlevel",
            "update",
            "force",
            "passthru",
        ],
        _ => return None,
    };
    Some(names)
}

/// The lowercase name a parameter token selects among `names` (plus the common
/// parameters), or every name it abbreviates when that is not exactly one.
pub(super) fn candidates(token: &str, names: &[&'static str]) -> Vec<&'static str> {
    let key = token
        .trim_start_matches('-')
        .trim_end_matches(':')
        .to_ascii_lowercase();
    if key.is_empty() {
        return Vec::new();
    }
    let all = || names.iter().chain(COMMON).copied();
    if let Some(name) = all().find(|name| *name == key) {
        return vec![name];
    }
    if let Some((_, name)) = COMMON_ALIASES.iter().find(|(alias, _)| *alias == key) {
        return vec![*name];
    }
    let mut found: Vec<_> = all().filter(|name| name.starts_with(&key)).collect();
    found.dedup();
    found
}

/// The one parameter a token selects, if it selects exactly one.
pub(super) fn resolve(token: &str, names: &[&'static str]) -> Option<&'static str> {
    match candidates(token, names).as_slice() {
        [name] => Some(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REMOVE: &[&str] = &["path", "literalpath", "filter", "recurse", "force"];

    #[test]
    fn exact_names_aliases_and_unique_prefixes_resolve() {
        assert_eq!(resolve("-Recurse", REMOVE), Some("recurse"));
        assert_eq!(resolve("-rec", REMOVE), Some("recurse"));
        assert_eq!(resolve("-Fo", REMOVE), Some("force"));
        assert_eq!(resolve("-LP", REMOVE), Some("literalpath"));
        assert_eq!(resolve("-Path:", REMOVE), Some("path"));
        assert_eq!(resolve("-EA", REMOVE), Some("erroraction"));
    }

    #[test]
    fn ambiguous_and_unknown_tokens_resolve_to_nothing() {
        assert_eq!(resolve("-f", REMOVE), None);
        assert_eq!(candidates("-f", REMOVE), ["filter", "force"]);
        assert_eq!(resolve("-bogus", REMOVE), None);
        assert_eq!(resolve("-", REMOVE), None);
    }
}
