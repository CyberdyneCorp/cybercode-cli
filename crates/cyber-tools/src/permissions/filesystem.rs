//! Conservative proof for automatic filesystem edits in accept-edits mode.

use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

use super::removals::literal;
use crate::{bash_analysis::normalize, host::canonical};

/// Unknown syntax or option semantics keeps ordinary permission handling.
pub(crate) fn literal_edits(source: &str, cwd: &Path) -> Option<Vec<PathBuf>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(source, None)?;
    if tree.root_node().has_error() {
        return None;
    }
    let mut paths = Vec::new();
    collect(tree.root_node(), source.as_bytes(), cwd, &mut paths)?;
    (!paths.is_empty()).then_some(paths)
}

fn collect(node: Node<'_>, src: &[u8], cwd: &Path, paths: &mut Vec<PathBuf>) -> Option<()> {
    match node.kind() {
        "program" | "list" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    collect(child, src, cwd, paths)?;
                } else if child.kind() == "&" {
                    return None;
                }
            }
            Some(())
        }
        "comment" => Some(()),
        "command" => command(node, src, cwd, paths),
        _ => None,
    }
}

fn command(node: Node<'_>, src: &[u8], cwd: &Path, paths: &mut Vec<PathBuf>) -> Option<()> {
    let mut cursor = node.walk();
    let words: Vec<String> = node
        .named_children(&mut cursor)
        .map(|word| match word.kind() {
            "command_name" | "word" | "string" | "raw_string" | "concatenation" | "number" => {
                literal(word, src)
            }
            _ => None,
        })
        .collect::<Option<_>>()?;
    let (name, args) = words.split_first()?;
    let operands = operands(name, args)?;
    let resolved: Vec<PathBuf> = operands
        .iter()
        .map(|word| normalize(&canonical(&cwd.join(word))))
        .collect();
    if matches!(name.as_str(), "cp" | "mv") {
        copy_targets(&resolved, &operands, paths)?;
    }
    paths.extend(resolved);
    Some(())
}

fn operands<'a>(name: &str, args: &'a [String]) -> Option<Vec<&'a str>> {
    let flags = match name {
        "mkdir" => "pv",
        "touch" => "acm",
        "cp" => "pfinv",
        "mv" => "finv",
        "rm" => "fivd",
        _ => return None,
    };
    let mut paths = Vec::new();
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
        } else if options && arg.starts_with('-') {
            known_option(name, arg, flags)?;
        } else {
            // Bare tilde expansion is not a literal path. Quoted tilde is also
            // conservatively left for ordinary approval instead of guessing.
            if arg.is_empty() || arg.starts_with('~') {
                return None;
            }
            paths.push(arg.as_str());
        }
    }
    let minimum = if matches!(name, "cp" | "mv") { 2 } else { 1 };
    (paths.len() >= minimum).then_some(paths)
}

fn known_option(name: &str, arg: &str, flags: &str) -> Option<()> {
    let long = match name {
        "mkdir" => ["--parents", "--verbose"].as_slice(),
        "touch" => ["--no-create"].as_slice(),
        "cp" | "mv" | "rm" => ["--force", "--interactive", "--verbose"].as_slice(),
        _ => return None,
    };
    let short = arg.strip_prefix('-').filter(|s| !s.is_empty())?;
    (long.contains(&arg) || short.chars().all(|c| flags.contains(c))).then_some(())
}

fn copy_targets(operands: &[PathBuf], names: &[&str], paths: &mut Vec<PathBuf>) -> Option<()> {
    let (destination, sources) = operands.split_last()?;
    // Moving directories can implicitly mutate protected descendants. Recursive
    // copies and directory moves need separate proof before automatic approval.
    if sources.iter().any(|path| path.is_dir()) {
        return None;
    }
    if destination.is_dir() {
        for source in &names[..names.len() - 1] {
            let target = destination.join(Path::new(source).file_name()?);
            paths.push(normalize(&canonical(&target)));
        }
    } else if sources.len() != 1 {
        return None;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_lists_quotes_and_option_delimiters_are_proven() {
        let paths = literal_edits(
            "mkdir -pv build && touch 'build/a b'; cp 'build/a b' build/c; mv build/c build/d; rm -- -r",
            Path::new("/repo"),
        ).unwrap();
        assert!(paths.contains(&PathBuf::from("/repo/build/a b")));
        assert!(paths.contains(&PathBuf::from("/repo/-r")));
        assert!(literal_edits("'touch' escaped\\ space", Path::new("/repo")).is_some());
    }

    #[test]
    fn unknown_execution_contexts_and_options_keep_ordinary_approval() {
        for source in [
            "DEST=/tmp/out; touch \"$DEST\"",
            "touch $DEST",
            "touch ~/out",
            "touch $(pwd)/out",
            "touch *.txt",
            "touch a > /tmp/out",
            "touch a | touch b",
            "(touch a)",
            "touch(){ mkdir outside; }; touch a",
            "command touch a",
            "env touch a",
            "touch -r /tmp/source local",
            "cp --target-directory=/tmp a",
            "cp -t /tmp a",
            "rm -rf build",
            "rm --recursive build",
            "mkdir --mode=700 build",
            "touch 'unterminated",
            "touch <(touch /tmp/out)",
            "touch a &",
            "touch",
        ] {
            assert_eq!(literal_edits(source, Path::new("/repo")), None, "{source}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_targets_and_copy_destinations_are_resolved() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir(root.join("dest")).unwrap();
        std::fs::write(root.join("secret"), "original").unwrap();
        std::os::unix::fs::symlink(root.join("secret"), root.join("dest/a")).unwrap();
        let paths = literal_edits("cp a dest", &root).unwrap();
        assert!(paths.contains(&root.join("secret")));
        std::os::unix::fs::symlink(root.join("dest"), root.join("alias")).unwrap();
        let paths = literal_edits("touch alias/../new", &root).unwrap();
        assert_eq!(paths, vec![root.join("new")]);
        assert!(literal_edits("mv dest renamed", &root).is_none());
    }
}
