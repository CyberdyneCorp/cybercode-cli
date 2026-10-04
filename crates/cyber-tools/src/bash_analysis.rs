//! Bash permission analysis (`builtin-tools` → Bash permission analysis).
//!
//! Each simple command, including commands inside pipelines, lists, subshells and command
//! substitutions, is a separate permission resource. The "always" pattern is the command's
//! arity prefix plus ` *`. Unparseable input is one resource that prefix rules never allow.

use std::path::{Path, PathBuf};

use tree_sitter::{Node, Parser};

/// Commands whose second word is a subcommand, so the arity prefix has two words.
const SUBCOMMAND_TOOLS: &[&str] = &[
    "git",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "cargo",
    "go",
    "docker",
    "kubectl",
    "gh",
    "pip",
    "pip3",
    "uv",
    "poetry",
    "deno",
    "dotnet",
    "brew",
    "apt",
    "apt-get",
    "helm",
    "terraform",
    "make",
    "just",
    "rustup",
    "python",
    "python3",
];

/// Commands whose path arguments are mutated.
const MUTATING: &[&str] = &[
    "rm", "mv", "cp", "mkdir", "touch", "chmod", "chown", "ln", "tee", "rmdir", "truncate",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    /// Source text, the permission resource.
    pub text: String,
    /// Pattern offered for an `always` approval, for example `git commit *`.
    pub always: String,
    /// Absolute paths the command mutates (arguments of mutating commands and redirections).
    pub mutates: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub commands: Vec<SimpleCommand>,
    /// The command could not be parsed; treat it as one opaque resource.
    pub unparseable: bool,
}

pub fn analyze(command: &str, cwd: &Path) -> Analysis {
    let mut parser = Parser::new();
    let parsed = parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .ok()
        .and_then(|()| parser.parse(command, None));
    let Some(tree) = parsed.filter(|t| !t.root_node().has_error()) else {
        return opaque(command);
    };
    let mut commands = Vec::new();
    collect(tree.root_node(), command.as_bytes(), cwd, &mut commands);
    if commands.is_empty() {
        return opaque(command);
    }
    Analysis {
        commands,
        unparseable: false,
    }
}

fn opaque(command: &str) -> Analysis {
    let text = command.trim().to_string();
    // The exact text is the only pattern: prefix rules never match unparseable input.
    Analysis {
        commands: vec![SimpleCommand {
            always: text.clone(),
            text,
            mutates: Vec::new(),
        }],
        unparseable: true,
    }
}

fn collect(node: Node<'_>, src: &[u8], cwd: &Path, out: &mut Vec<SimpleCommand>) {
    if node.kind() == "command" {
        out.push(simple(node, src, cwd));
    }
    let targets = if node.kind() == "redirected_statement" {
        redirect_targets(node, src, cwd)
    } else {
        Vec::new()
    };
    let before = out.len();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // Recurse into commands too: command substitutions nest further commands.
        collect(child, src, cwd, out);
    }
    // In `a && b > f` the grammar wraps the whole list, but bash redirects only `b`.
    if out.len() > before
        && let Some(last) = out.last_mut()
    {
        last.mutates.extend(targets);
    }
}

fn simple(node: Node<'_>, src: &[u8], cwd: &Path) -> SimpleCommand {
    let text = node.utf8_text(src).unwrap_or_default().trim().to_string();
    let words = words(node, src);
    let name = words.first().map(String::as_str).unwrap_or_default();
    let arity = if SUBCOMMAND_TOOLS.contains(&name) {
        2
    } else {
        1
    };
    let always = format!(
        "{} *",
        words
            .iter()
            .take(arity)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    );
    let mutates = if MUTATING.contains(&name) {
        words
            .iter()
            .skip(1)
            .filter(|w| !w.starts_with('-'))
            .map(|w| resolve(cwd, w))
            .collect()
    } else {
        Vec::new()
    };
    SimpleCommand {
        text,
        always,
        mutates,
    }
}

/// The command name and argument words, with surrounding quotes removed.
fn words(node: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| {
            matches!(
                c.kind(),
                "command_name" | "word" | "string" | "raw_string" | "concatenation" | "number"
            )
        })
        .filter_map(|c| c.utf8_text(src).ok())
        .map(|w| w.trim_matches(|ch| ch == '"' || ch == '\'').to_string())
        .collect()
}

fn redirect_targets(node: Node<'_>, src: &[u8], cwd: &Path) -> Vec<PathBuf> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| c.kind() == "file_redirect")
        .filter_map(|r| {
            let op = r.child(0)?.utf8_text(src).ok()?;
            let output = op.contains('>');
            let target = r.child_by_field_name("destination")?.utf8_text(src).ok()?;
            (output && target != "/dev/null" && !target.starts_with('&'))
                .then(|| resolve(cwd, target.trim_matches('"')))
        })
        .collect()
}

fn resolve(cwd: &Path, word: &str) -> PathBuf {
    let path = Path::new(word);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    normalize(&joined)
}

/// Lexically normalize `.` and `..` without touching the filesystem.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(cmd: &str) -> Vec<String> {
        analyze(cmd, Path::new("/repo"))
            .commands
            .into_iter()
            .map(|c| c.text)
            .collect()
    }

    #[test]
    fn compound_commands_are_split() {
        assert_eq!(texts("npm test && git push"), vec!["npm test", "git push"]);
        assert_eq!(
            texts("cat a | grep b; echo $(whoami)"),
            vec!["cat a", "grep b", "echo $(whoami)", "whoami"]
        );
        assert_eq!(texts("(cd sub && make)"), vec!["cd sub", "make"]);
    }

    #[test]
    fn always_patterns_use_the_arity_prefix() {
        let a = analyze(
            "git commit -m 'x' && npm run build && ls -la",
            Path::new("/repo"),
        );
        let always: Vec<&str> = a.commands.iter().map(|c| c.always.as_str()).collect();
        assert_eq!(always, vec!["git commit *", "npm run *", "ls *"]);
    }

    #[test]
    fn mutated_paths_include_arguments_and_redirections() {
        let a = analyze(
            "rm -rf ../outside build && echo hi > /tmp/out.txt",
            Path::new("/repo"),
        );
        assert_eq!(
            a.commands[0].mutates,
            vec![PathBuf::from("/outside"), PathBuf::from("/repo/build")]
        );
        assert_eq!(a.commands[1].mutates, vec![PathBuf::from("/tmp/out.txt")]);
        assert!(
            analyze("ls > /dev/null 2>&1", Path::new("/repo")).commands[0]
                .mutates
                .is_empty()
        );
    }

    #[test]
    fn unparseable_input_is_one_opaque_resource() {
        let a = analyze("echo 'unterminated", Path::new("/repo"));
        assert!(a.unparseable);
        assert_eq!(a.commands.len(), 1);
        assert_eq!(
            a.commands[0].always, a.commands[0].text,
            "no prefix pattern"
        );
    }
}
