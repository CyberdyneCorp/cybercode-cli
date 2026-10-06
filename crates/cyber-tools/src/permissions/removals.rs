//! Critical Bash removal analysis. Other inline languages are a separate M1.1 task.

use std::path::{Path, PathBuf};

use tree_sitter::{Node, Parser};

use crate::bash_analysis::normalize;

mod input;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalRisk {
    Critical(PathBuf),
    Unresolved(String),
}

impl RemovalRisk {
    pub fn refusal(&self) -> String {
        match self {
            Self::Critical(path) => format!(
                "Refused: removal of critical path {}. Rewrite the command to target specific files.",
                path.display()
            ),
            Self::Unresolved(reason) => format!(
                "Refused: removal target cannot be resolved safely ({reason}). Rewrite the command to target specific files."
            ),
        }
    }

    pub fn warning(&self) -> String {
        match self {
            Self::Critical(path) => format!(
                "Danger: this command can remove critical path {} and its contents. Confirmation is required every time.",
                path.display()
            ),
            Self::Unresolved(reason) => format!(
                "Danger: the removal target cannot be resolved safely ({reason}); it may include a critical path."
            ),
        }
    }
}

pub struct RemovalScope<'a> {
    pub location: &'a Path,
    pub project: &'a Path,
    pub home: &'a Path,
    pub workdir: &'a Path,
}

const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh"];
const MAX_NESTING: usize = 8;

/// Return critical roots before unresolved targets, so a known dangerous action gets a
/// concrete explanation even when another action in the same command is ambiguous.
pub fn bash_removal(command: &str, scope: &RemovalScope<'_>) -> Option<RemovalRisk> {
    let mut risks = Vec::new();
    inspect(command, scope, 0, false, &mut risks);
    risks
        .iter()
        .find(|r| matches!(r, RemovalRisk::Critical(_)))
        .cloned()
        .or_else(|| risks.into_iter().next())
}

fn inspect(
    command: &str,
    scope: &RemovalScope<'_>,
    depth: usize,
    inherited_directory_change: bool,
    risks: &mut Vec<RemovalRisk>,
) {
    if depth > MAX_NESTING {
        risks.push(RemovalRisk::Unresolved(
            "nested shell limit exceeded".into(),
        ));
        return;
    }
    let mut parser = Parser::new();
    let tree = parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .ok()
        .and_then(|()| parser.parse(command, None));
    let Some(tree) = tree.filter(|t| !t.root_node().has_error()) else {
        risks.push(RemovalRisk::Unresolved("unparseable shell command".into()));
        return;
    };
    let mut changed_directory = inherited_directory_change;
    walk(
        tree.root_node(),
        command.as_bytes(),
        scope,
        depth,
        &mut changed_directory,
        risks,
    );
}

fn walk(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    changed_directory: &mut bool,
    risks: &mut Vec<RemovalRisk>,
) {
    input::inspect_input(node, source, scope, depth, *changed_directory, risks);
    if node.kind() == "command" {
        inspect_command(node, source, scope, depth, changed_directory, risks);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, source, scope, depth, changed_directory, risks);
    }
}

fn inspect_command(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    changed_directory: &mut bool,
    risks: &mut Vec<RemovalRisk>,
) {
    let mut cursor = node.walk();
    let nodes: Vec<_> = node
        .named_children(&mut cursor)
        .filter(|n| n.kind() != "variable_assignment" && !n.kind().ends_with("_redirect"))
        .collect();
    let Some(name) = nodes.first().and_then(|n| literal(*n, source)) else {
        risks.push(RemovalRisk::Unresolved("dynamic shell command name".into()));
        return;
    };
    let name = Path::new(&name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&name);
    let args = &nodes[1..];
    match name {
        "cd" | "pushd" | "popd" => *changed_directory = true,
        "rm" | "rmdir" | "unlink" => {
            inspect_removal(args, source, scope, *changed_directory, risks);
        }
        "find" => inspect_find(args, source, scope, *changed_directory, risks),
        "eval" => inspect_eval(args, source, scope, depth, *changed_directory, risks),
        shell if SHELLS.contains(&shell) => {
            inspect_shell(args, source, scope, depth, *changed_directory, risks);
        }
        "command" | "builtin" | "exec" | "env" | "nohup" => {
            inspect_wrapper(name, args, source, scope, depth, *changed_directory, risks);
        }
        _ => {}
    }
}

fn inspect_removal(
    args: &[Node<'_>],
    source: &[u8],
    scope: &RemovalScope<'_>,
    changed_directory: bool,
    risks: &mut Vec<RemovalRisk>,
) {
    let mut options = true;
    for node in args {
        let value = literal(*node, source);
        if options && value.as_deref() == Some("--") {
            options = false;
            continue;
        }
        if options && value.is_some_and(|s| s.starts_with('-')) {
            continue;
        }
        inspect_target(*node, source, scope, changed_directory, risks);
    }
}

fn inspect_shell(
    args: &[Node<'_>],
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    changed_directory: bool,
    risks: &mut Vec<RemovalRisk>,
) {
    let flag = args.iter().position(|n| {
        literal(*n, source)
            .is_some_and(|s| s.starts_with('-') && !s.starts_with("--") && s.contains('c'))
    });
    if let Some(index) = flag {
        inspect_script(
            args.get(index + 1).copied(),
            source,
            scope,
            depth,
            changed_directory,
            risks,
        );
    }
}

fn inspect_script(
    node: Option<Node<'_>>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    changed_directory: bool,
    risks: &mut Vec<RemovalRisk>,
) {
    if let Some(script) = node.and_then(|n| literal(n, source)) {
        inspect(&script, scope, depth + 1, changed_directory, risks);
    } else {
        risks.push(RemovalRisk::Unresolved("dynamic shell script".into()));
    }
}

fn inspect_eval(
    args: &[Node<'_>],
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    changed_directory: bool,
    risks: &mut Vec<RemovalRisk>,
) {
    let parts: Option<Vec<_>> = args.iter().map(|n| literal(*n, source)).collect();
    match parts {
        Some(parts) => inspect(&parts.join(" "), scope, depth + 1, changed_directory, risks),
        _ => risks.push(RemovalRisk::Unresolved(
            "dynamic eval or directory change".into(),
        )),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum WrapperCommand {
    Run {
        index: usize,
        changes_directory: bool,
    },
    Lookup,
    Unresolved,
}

fn wrapper_command(name: &str, values: &[Option<String>]) -> WrapperCommand {
    let mut index = 0;
    let mut changes_directory = false;
    while let Some(value) = values.get(index) {
        let Some(value) = value.as_deref() else {
            return WrapperCommand::Unresolved;
        };
        if value == "--" {
            index += 1;
            break;
        }
        if name == "env" && value.contains('=') && !value.starts_with('-') {
            index += 1;
            continue;
        }
        if !value.starts_with('-') || value == "-" {
            break;
        }
        match wrapper_option(name, value) {
            WrapperOption::Lookup => return WrapperCommand::Lookup,
            WrapperOption::Unknown => return WrapperCommand::Unresolved,
            WrapperOption::Flag => index += 1,
            WrapperOption::Value { directory } => {
                if values.get(index + 1).and_then(|v| v.as_ref()).is_none() {
                    return WrapperCommand::Unresolved;
                }
                changes_directory |= directory;
                index += 2;
            }
            WrapperOption::Attached { directory } => {
                changes_directory |= directory;
                index += 1;
            }
        }
    }
    if index == values.len() {
        WrapperCommand::Lookup
    } else {
        WrapperCommand::Run {
            index,
            changes_directory,
        }
    }
}

enum WrapperOption {
    Flag,
    Value { directory: bool },
    Attached { directory: bool },
    Lookup,
    Unknown,
}

fn wrapper_option(name: &str, value: &str) -> WrapperOption {
    match (name, value) {
        ("command", "-v" | "-V" | "-pv" | "-pV") | ("env" | "nohup", "--help" | "--version") => {
            WrapperOption::Lookup
        }
        ("command", "-p")
        | ("exec", "-c" | "-l" | "-cl" | "-lc")
        | ("env", "-i" | "--ignore-environment" | "-0" | "--null") => WrapperOption::Flag,
        ("exec", "-a") | ("env", "-u" | "--unset") => WrapperOption::Value { directory: false },
        ("env", "-C" | "--chdir") => WrapperOption::Value { directory: true },
        ("env", v) if v.starts_with("--unset=") || v.starts_with("-u") => {
            WrapperOption::Attached { directory: false }
        }
        ("env", v) if v.starts_with("--chdir=") || v.starts_with("-C") => {
            WrapperOption::Attached { directory: true }
        }
        ("exec", v) if v.starts_with("-a") => WrapperOption::Attached { directory: false },
        _ => WrapperOption::Unknown,
    }
}

fn inspect_wrapper(
    name: &str,
    args: &[Node<'_>],
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    changed_directory: bool,
    risks: &mut Vec<RemovalRisk>,
) {
    let values: Vec<_> = args.iter().map(|n| literal(*n, source)).collect();
    let (index, changes_directory) = match wrapper_command(name, &values) {
        WrapperCommand::Run {
            index,
            changes_directory,
        } => (index, changes_directory),
        WrapperCommand::Lookup => return,
        WrapperCommand::Unresolved => {
            risks.push(RemovalRisk::Unresolved(
                "dynamic or unsupported shell wrapper".into(),
            ));
            return;
        }
    };
    // Preserve shell quoting by reparsing the source span, not joining decoded arguments.
    let source_slice = &source[args[index].start_byte()..args.last().unwrap().end_byte()];
    if let Ok(text) = std::str::from_utf8(source_slice) {
        inspect(
            text,
            scope,
            depth + 1,
            changed_directory || changes_directory,
            risks,
        );
    }
}

fn inspect_find(
    args: &[Node<'_>],
    source: &[u8],
    scope: &RemovalScope<'_>,
    changed_directory: bool,
    risks: &mut Vec<RemovalRisk>,
) {
    if !args
        .iter()
        .any(|n| literal(*n, source).as_deref() == Some("-delete"))
    {
        return;
    }
    let Some(start) = find_paths_start(args, source) else {
        risks.push(RemovalRisk::Unresolved("dynamic find option".into()));
        return;
    };
    let paths: Vec<_> = args[start..]
        .iter()
        .take_while(|n| {
            literal(**n, source).is_none_or(|s| !s.starts_with('-') && s != "!" && s != "(")
        })
        .collect();
    if paths.is_empty() {
        inspect_path(".", false, scope, changed_directory, risks);
    }
    for node in paths {
        inspect_target(*node, source, scope, changed_directory, risks);
    }
}

fn find_paths_start(args: &[Node<'_>], source: &[u8]) -> Option<usize> {
    let mut index = 0;
    while let Some(node) = args.get(index) {
        let value = literal(*node, source)?;
        match value.as_str() {
            "-H" | "-L" | "-P" => index += 1,
            "--" => return Some(index + 1),
            "-D" => {
                literal(*args.get(index + 1)?, source)?;
                index += 2;
            }
            v if v.starts_with("-O") => index += 1,
            _ => break,
        }
    }
    Some(index)
}

fn inspect_target(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    changed_directory: bool,
    risks: &mut Vec<RemovalRisk>,
) {
    let raw = node.utf8_text(source).unwrap_or_default();
    if matches!(
        raw,
        "\"$(git rev-parse --show-toplevel)\"" | "$(git rev-parse --show-toplevel)"
    ) {
        risks.push(RemovalRisk::Critical(scope.project.to_path_buf()));
        return;
    }
    let value = match raw {
        "$HOME" | "${HOME}" | "\"$HOME\"" | "\"${HOME}\"" => Some(scope.home.display().to_string()),
        "$PWD" | "${PWD}" | "\"$PWD\"" | "\"${PWD}\"" if !changed_directory => {
            Some(scope.workdir.display().to_string())
        }
        _ => literal(node, source),
    };
    match value {
        Some(path) => inspect_path(&path, raw.starts_with('~'), scope, changed_directory, risks),
        None => risks.push(RemovalRisk::Unresolved("dynamic removal argument".into())),
    }
}

fn inspect_path(
    text: &str,
    expand_home: bool,
    scope: &RemovalScope<'_>,
    changed_directory: bool,
    risks: &mut Vec<RemovalRisk>,
) {
    let path = if expand_home && text == "~" {
        scope.home.to_path_buf()
    } else if expand_home && text.starts_with("~/") {
        scope.home.join(&text[2..])
    } else if Path::new(text).is_absolute() {
        PathBuf::from(text)
    } else if changed_directory {
        risks.push(RemovalRisk::Unresolved(
            "relative removal after directory change".into(),
        ));
        return;
    } else {
        scope.workdir.join(text)
    };
    // Resolve symlinks before lexical `..`: link/.. refers to the target's parent.
    let alias = crate::host::canonical(&path);
    let path = normalize(&path);
    if critical(&path, scope) || critical(&alias, scope) {
        risks.push(RemovalRisk::Critical(alias));
    }
}

fn critical(path: &Path, scope: &RemovalScope<'_>) -> bool {
    let location = crate::host::canonical(scope.location);
    let home = crate::host::canonical(scope.home);
    let project = crate::host::canonical(scope.project);
    path.parent().is_none()
        || path == home
        || path == project
        || location.starts_with(path)
        || path == project.join(".git")
        || path == location.join(".git")
}

/// Decode a literal shell word; expansions, globbing and substitutions stay unresolved.
fn literal(node: Node<'_>, source: &[u8]) -> Option<String> {
    let raw = node.utf8_text(source).ok()?;
    let mut quote = None;
    let mut escaped = false;
    let mut out = String::new();
    for c in raw.chars() {
        if escaped {
            decode_escape(&mut out, quote, c);
            escaped = false;
            continue;
        }
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), c) => out.push(c),
            (_, '\\') => escaped = true,
            (None, '\'' | '"') => quote = Some(c),
            (_, '$' | '`') | (None, '*' | '?' | '[' | '{') => return None,
            (_, c) => out.push(c),
        }
    }
    (!escaped && quote.is_none()).then_some(out)
}

fn decode_escape(out: &mut String, quote: Option<char>, c: char) {
    if c == '\n' {
        return;
    }
    if quote == Some('"') && !matches!(c, '$' | '`' | '"' | '\\') {
        out.push('\\');
    }
    out.push(c);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> RemovalScope<'static> {
        RemovalScope {
            location: Path::new("/repo/sub"),
            project: Path::new("/repo"),
            home: Path::new("/home/user"),
            workdir: Path::new("/repo/sub"),
        }
    }

    #[test]
    fn critical_roots_and_location_ancestors_are_detected() {
        for command in [
            "rm -rf /",
            "rm -rf .",
            "rm -rf ..",
            "rm -- /repo",
            "rmdir /repo/sub",
            "unlink /repo/.git",
            "find /repo -delete",
            "find . -type f -delete",
            "find -delete",
            "rm -rf ~",
            "rm -rf $HOME",
            "rm -rf \"${HOME}\"",
            "rm -rf \"$PWD\"",
            "rm -rf \"$(git rev-parse --show-toplevel)\"",
        ] {
            assert!(
                matches!(
                    bash_removal(command, &scope()),
                    Some(RemovalRisk::Critical(_))
                ),
                "{command}"
            );
        }
    }

    #[test]
    fn safe_operands_and_quoted_text_are_not_mistaken_for_root_removals() {
        for command in [
            "rm build/file.txt",
            "rm -rf build",
            "rm -rf '/repo/sub/*'",
            "echo 'rm -rf /repo'",
            "printf '%s' '$HOME'",
            "rmdir empty",
            "rm -- -file",
            "find build -name '*.tmp' -delete",
            "rm '~'",
            "cd sub; sh -c 'echo done'",
        ] {
            assert_eq!(bash_removal(command, &scope()), None, "{command}");
        }
    }

    #[test]
    fn nested_shells_eval_and_substitutions_are_inspected() {
        for command in [
            "bash -c 'rm -rf /repo'",
            "sh -ec 'rm -rf /repo/sub'",
            "eval 'rm -rf /repo'",
            "command rm -rf /repo",
            "env A=1 bash -c 'rm -rf /repo'",
            "echo $(rm -rf /repo)",
            "(rm -rf /repo)",
            "f() { rm -rf /repo; }; f",
            "sh -c \"bash -c 'rm -rf /repo'\"",
        ] {
            assert!(
                matches!(
                    bash_removal(command, &scope()),
                    Some(RemovalRisk::Critical(_))
                ),
                "{command}"
            );
        }
    }

    #[test]
    fn wrapper_option_values_cannot_hide_removal_commands() {
        for command in [
            "env -u NAME rm -rf /repo",
            "env --unset NAME rm -rf /repo",
            "env --unset=NAME rm -rf /repo",
            "env -uNAME A=1 rm -rf /repo",
            "exec -a process-name rm -rf /repo",
            "exec -aprocess-name rm -rf /repo",
            "command -p -- rm -rf /repo",
            "builtin -- eval 'rm -rf /repo'",
            "nohup -- rm -rf /repo",
            "env -C /tmp rm -rf /repo",
        ] {
            assert_eq!(
                bash_removal(command, &scope()),
                Some(RemovalRisk::Critical("/repo".into())),
                "{command}"
            );
        }
        for command in [
            "env -C /tmp rm -rf ..",
            "env --chdir=/tmp rm -rf ..",
            "env -S 'rm -rf /repo'",
            r#"env -u "$NAME" rm -rf /repo"#,
        ] {
            assert!(
                matches!(
                    bash_removal(command, &scope()),
                    Some(RemovalRisk::Unresolved(_))
                ),
                "{command}"
            );
        }
    }

    #[test]
    fn wrapper_lookups_and_literal_equals_paths_are_distinct() {
        for command in [
            "command -v rm",
            "command -V rm",
            "command -pv rm",
            "env --help rm -rf /repo",
            "nohup --version rm -rf /repo",
            "exec rm build/name=value",
            "command rm build/name=value",
        ] {
            assert_eq!(bash_removal(command, &scope()), None, "{command}");
        }
        assert_eq!(
            bash_removal("exec rm name=value /repo", &scope()),
            Some(RemovalRisk::Critical("/repo".into()))
        );
    }

    #[test]
    fn find_global_options_preserve_explicit_and_implicit_search_roots() {
        for command in [
            "find -L build -delete",
            "find -H -P build -delete",
            "find -O2 build -delete",
            "find -D tree build -delete",
        ] {
            assert_eq!(bash_removal(command, &scope()), None, "{command}");
        }
        for command in [
            "find -L /repo -delete",
            "find -D tree /repo -delete",
            "find -- /repo -delete",
        ] {
            assert_eq!(
                bash_removal(command, &scope()),
                Some(RemovalRisk::Critical("/repo".into())),
                "{command}"
            );
        }
        assert_eq!(
            bash_removal("find -L -delete", &scope()),
            Some(RemovalRisk::Critical(scope().workdir.to_path_buf()))
        );
    }

    #[test]
    fn shell_stdin_is_code_but_ordinary_heredoc_data_is_not() {
        for command in [
            "bash <<'EOF'\nrm -rf /repo\nEOF\n",
            "sh -s argument <<EOF\nrm -rf /repo\nEOF\n",
            "env -u NAME bash <<'EOF'\nrm -rf /repo\nEOF\n",
            "bash <<< 'rm -rf /repo'",
            "bash 0<<< 'rm -rf /repo'",
            "cat <<< 'rm -rf /repo' | bash",
            "bash <<'EOF' | cat\nrm -rf /repo\nEOF\n",
            "cat <<'EOF' | bash\nrm -rf /repo\nEOF\n",
            "cat <<EOF\n$(rm -rf /repo)\nEOF\n",
        ] {
            assert_eq!(
                bash_removal(command, &scope()),
                Some(RemovalRisk::Critical("/repo".into())),
                "{command}"
            );
        }
        for command in [
            "cat <<'EOF'\nrm -rf /repo\nEOF\n",
            "cat <<'EOF'\n$(rm -rf /repo)\nEOF\n",
            "bash -c 'echo done' <<'EOF'\nrm -rf /repo\nEOF\n",
            "bash script.sh <<'EOF'\nrm -rf /repo\nEOF\n",
            "bash <<'EOF'\necho 'rm -rf /repo'\nEOF\n",
            "cat <<< 'rm -rf /repo'",
            "cat <<< 'rm -rf /repo' | grep rm",
            "rm build/file <<< 'rm -rf /repo'",
        ] {
            assert_eq!(bash_removal(command, &scope()), None, "{command}");
        }
        for command in [
            "bash <<EOF\n$COMMAND\nEOF\n",
            r#"bash <<< "$COMMAND""#,
            "bash <<'EOF'\n$COMMAND /repo\nEOF\n",
            "env -C /tmp bash <<'EOF'\nrm -rf ..\nEOF\n",
            "bash 3<<< 'echo done' <&3",
            "bash 0<<'EOF'\nrm -rf /repo\nEOF\n",
        ] {
            assert!(
                matches!(
                    bash_removal(command, &scope()),
                    Some(RemovalRisk::Unresolved(_))
                ),
                "{command}"
            );
        }
    }

    #[test]
    fn ambiguous_targets_and_changed_directory_never_bypass_the_guard() {
        for command in [
            "rm -rf $TARGET",
            "rm -rf build/*",
            "rm -rf $(unknown)",
            "cd sub && rm -rf ..",
            "bash -c \"$SCRIPT\"",
            "eval \"$COMMAND\"",
            "rm 'unterminated",
        ] {
            assert!(
                matches!(
                    bash_removal(command, &scope()),
                    Some(RemovalRisk::Unresolved(_))
                ),
                "{command}"
            );
        }
        assert_eq!(
            bash_removal("cd sub; rm -rf /repo", &scope()),
            Some(RemovalRisk::Critical("/repo".into()))
        );
    }

    #[test]
    fn escaped_spaces_and_concatenated_quotes_preserve_the_target() {
        let scope = RemovalScope {
            location: Path::new("/repo space"),
            project: Path::new("/repo space"),
            ..scope()
        };
        for command in [
            "rm -rf /repo\\ space",
            "rm -rf '/repo 'space",
            "rm -rf \"/repo space\"",
        ] {
            assert_eq!(
                bash_removal(command, &scope),
                Some(RemovalRisk::Critical("/repo space".into())),
                "{command}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_cannot_hide_a_critical_root() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&repo, &alias).unwrap();
        let scope = RemovalScope {
            location: &repo,
            project: &repo,
            home: Path::new("/home/user"),
            workdir: &repo,
        };
        let command = format!("rm -rf '{}/.'", alias.display());
        assert_eq!(
            bash_removal(&command, &scope),
            Some(RemovalRisk::Critical(std::fs::canonicalize(&repo).unwrap()))
        );
        assert!(repo.exists(), "analysis must never execute the command");
        let child = repo.join("child");
        std::fs::create_dir(&child).unwrap();
        std::os::unix::fs::symlink(&repo, child.join("link")).unwrap();
        assert_eq!(
            bash_removal("rm -rf child/link/..", &scope),
            Some(RemovalRisk::Critical(
                std::fs::canonicalize(temp.path()).unwrap()
            )),
            "symlink traversal must happen before collapsing parent components"
        );
    }
}
