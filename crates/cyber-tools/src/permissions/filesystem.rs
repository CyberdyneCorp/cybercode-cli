//! Conservative proof for automatic filesystem edits in accept-edits mode.

use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

use super::removals::literal;
use crate::{bash_analysis::normalize, host::canonical};

mod links;

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
    if matches!(name.as_str(), "cp" | "mv") {
        for operand in &operands[..operands.len() - 1] {
            if std::fs::symlink_metadata(cwd.join(operand))
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return None;
            }
        }
    }
    let resolved: Vec<PathBuf> = operands
        .iter()
        .map(|word| literal_path(&cwd.join(word)))
        .collect::<Option<_>>()?;
    if resolved.iter().any(|path| !links::unaliased(path)) {
        return None;
    }
    if matches!(name.as_str(), "cp" | "mv") {
        copy_targets(name, args, &resolved, &operands, paths)?;
    }
    paths.extend(resolved);
    Some(())
}

fn operands<'a>(name: &str, args: &'a [String]) -> Option<Vec<&'a str>> {
    let flags = match name {
        "mkdir" => "pv",
        "touch" => "acm",
        "cp" => "pfinvrR",
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
        "cp" => ["--force", "--interactive", "--verbose", "--recursive"].as_slice(),
        "mv" | "rm" => ["--force", "--interactive", "--verbose"].as_slice(),
        _ => return None,
    };
    let short = arg.strip_prefix('-').filter(|s| !s.is_empty())?;
    (long.contains(&arg) || short.chars().all(|c| flags.contains(c))).then_some(())
}

fn copy_targets(
    name: &str,
    args: &[String],
    operands: &[PathBuf],
    names: &[&str],
    paths: &mut Vec<PathBuf>,
) -> Option<()> {
    let (destination, sources) = operands.split_last()?;
    if !destination.is_dir() && sources.len() != 1 {
        return None;
    }
    let recursive = name == "mv" || recursive_copy(args);
    let mut remaining = TREE_LIMIT;
    for (source, spelling) in sources.iter().zip(names) {
        let target = if destination.is_dir() {
            destination.join(Path::new(spelling).file_name()?)
        } else {
            destination.clone()
        };
        let target = literal_path(&target)?;
        if !links::unaliased(&target) {
            return None;
        }
        paths.push(target.clone());
        if source.is_dir() {
            // Trailing slash and dot operands have different copy semantics on
            // supported platforms; don't infer which tree layout they produce.
            if !recursive
                || spelling.ends_with(['/', '\\'])
                || matches!(spelling.rsplit(['/', '\\']).next(), Some("." | ".."))
                || Path::new(spelling).file_name().is_none()
            {
                return None;
            }
            directory_targets(source, &target, paths, &mut remaining)?;
        }
    }
    Some(())
}

fn recursive_copy(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--recursive" || (arg.starts_with('-') && arg.contains(['r', 'R'])))
}

const TREE_LIMIT: usize = 4096;

fn directory_targets(
    source: &Path,
    target: &Path,
    paths: &mut Vec<PathBuf>,
    remaining: &mut usize,
) -> Option<()> {
    let mut pending = vec![(source.to_owned(), target.to_owned())];
    while let Some((source, target)) = pending.pop() {
        *remaining = remaining.checked_sub(1)?;
        let kind = std::fs::symlink_metadata(&source).ok()?.file_type();
        if kind.is_symlink() || !(kind.is_file() || kind.is_dir()) {
            return None;
        }
        let destination = literal_path(&target)?;
        // A second file name may alias protected data outside this tree.
        if !links::unaliased(&source) || !links::unaliased(&destination) {
            return None;
        }
        paths.push(normalize(&canonical(&source)));
        paths.push(destination);
        if kind.is_dir() {
            for entry in std::fs::read_dir(&source).ok()? {
                let entry = entry.ok()?;
                if pending.len() >= *remaining {
                    return None;
                }
                pending.push((entry.path(), target.join(entry.file_name())));
            }
        }
    }
    Some(())
}

fn literal_path(path: &Path) -> Option<PathBuf> {
    let mut ancestor = path;
    let mut tail = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                // A dangling symlink is present even though exists() is false.
                let mut resolved = std::fs::canonicalize(ancestor).ok()?;
                for name in tail.into_iter().rev() {
                    resolved.push(name);
                }
                return Some(normalize(&resolved));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tail.push(ancestor.file_name()?);
                ancestor = ancestor.parent()?;
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_lists_quotes_and_option_delimiters_are_proven() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let paths = literal_edits(
            "mkdir -pv build && touch 'build/a b'; cp 'build/a b' build/c; mv build/c build/d; rm -- -r",
            &root,
        ).unwrap();
        assert!(paths.contains(&root.join("build/a b")));
        assert!(paths.contains(&root.join("-r")));
        assert!(literal_edits("'touch' escaped\\ space", &root).is_some());
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

    #[test]
    fn directory_layouts_include_every_source_and_destination_descendant() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir_all(root.join("source/nested")).unwrap();
        std::fs::write(root.join("source/nested/data"), "original").unwrap();
        std::fs::create_dir(root.join("destination")).unwrap();
        for command in [
            "cp -R source destination",
            "cp -r source destination",
            "cp --recursive source destination",
            "mv source destination",
        ] {
            let paths = literal_edits(command, &root).unwrap();
            assert!(
                paths.contains(&root.join("source/nested/data")),
                "{command}"
            );
            assert!(
                paths.contains(&root.join("destination/source/nested/data")),
                "{command}"
            );
        }
        let paths = literal_edits("cp -R source fresh", &root).unwrap();
        assert!(paths.contains(&root.join("fresh/nested/data")));
        for command in [
            "cp source fresh",
            "cp -RL source fresh",
            "cp -R source/ destination",
            "cp -R source/. destination",
            "mv source/ destination",
        ] {
            assert!(literal_edits(command, &root).is_none(), "{command}");
        }
    }

    #[test]
    fn directory_inventory_is_bounded_across_sources() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir(root.join("source")).unwrap();
        for index in 0..TREE_LIMIT {
            std::fs::write(root.join("source").join(index.to_string()), "x").unwrap();
        }
        assert!(literal_edits("cp -R source fresh", &root).is_none());
        std::fs::remove_file(root.join("source/0")).unwrap();
        assert!(literal_edits("cp -R source fresh", &root).is_some());
        std::fs::create_dir(root.join("second")).unwrap();
        std::fs::create_dir(root.join("destination")).unwrap();
        assert!(literal_edits("cp -R source second destination", &root).is_none());
    }

    #[test]
    fn directory_hard_links_keep_ordinary_approval() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir(root.join("source")).unwrap();
        std::fs::write(root.join("source/data"), "original").unwrap();
        std::fs::write(root.join("protected"), "protected").unwrap();
        std::fs::create_dir_all(root.join("destination/source")).unwrap();
        std::fs::hard_link(root.join("protected"), root.join("destination/source/data")).unwrap();
        assert!(literal_edits("cp -R source destination", &root).is_none());
        std::fs::hard_link(root.join("source/data"), root.join("alias")).unwrap();
        assert!(literal_edits("cp -R source fresh", &root).is_none());
        assert!(literal_edits("mv source fresh", &root).is_none());
        assert_eq!(
            std::fs::read_to_string(root.join("protected")).unwrap(),
            "protected"
        );
    }

    #[test]
    fn file_hard_links_and_implicit_copy_targets_keep_ordinary_approval() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::write(root.join("protected"), "protected").unwrap();
        std::fs::write(root.join("replacement"), "replacement").unwrap();
        std::fs::hard_link(root.join("protected"), root.join("alias")).unwrap();
        std::fs::create_dir(root.join("destination")).unwrap();
        std::fs::hard_link(root.join("protected"), root.join("destination/replacement")).unwrap();
        for command in [
            "cp replacement alias",
            "cp replacement destination",
            "touch alias",
            "mv alias fresh",
            "rm alias",
        ] {
            assert!(literal_edits(command, &root).is_none(), "{command}");
        }
        assert!(literal_edits("cp replacement fresh", &root).is_some());
        assert!(literal_edits("touch fresh", &root).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_file_operands_and_implicit_copy_targets_keep_ordinary_approval() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::write(root.join("replacement"), "replacement").unwrap();
        std::os::unix::fs::symlink(root.join("absent"), root.join("alias")).unwrap();
        std::fs::create_dir(root.join("destination")).unwrap();
        std::os::unix::fs::symlink(root.join("absent"), root.join("destination/replacement"))
            .unwrap();
        for command in [
            "touch alias",
            "mkdir -p alias/nested",
            "cp replacement alias",
            "cp replacement destination",
        ] {
            assert!(literal_edits(command, &root).is_none(), "{command}");
        }
        assert!(!root.join("absent").exists());
    }

    #[cfg(unix)]
    #[test]
    fn directory_source_links_refuse_and_destination_links_are_resolved() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir_all(root.join("source/nested")).unwrap();
        std::fs::write(root.join("source/nested/data"), "original").unwrap();
        std::fs::create_dir_all(root.join("destination/source")).unwrap();
        std::fs::create_dir(root.join("outside")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("destination/source/nested"))
            .unwrap();
        let paths = literal_edits("cp -R source destination", &root).unwrap();
        assert!(paths.contains(&root.join("outside/data")));
        std::os::unix::fs::symlink(root.join("source"), root.join("alias")).unwrap();
        assert!(literal_edits("cp -R alias fresh", &root).is_none());
        std::os::unix::fs::symlink(root.join("outside"), root.join("source/link")).unwrap();
        assert!(literal_edits("cp -R source fresh", &root).is_none());
        assert!(literal_edits("mv source fresh", &root).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_directory_destinations_keep_ordinary_approval() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir_all(root.join("source/nested")).unwrap();
        std::fs::write(root.join("source/nested/data"), "original").unwrap();
        std::fs::create_dir_all(root.join("destination/source")).unwrap();
        std::os::unix::fs::symlink(root.join("absent"), root.join("destination/source/nested"))
            .unwrap();
        assert!(literal_edits("cp -R source destination", &root).is_none());
        assert!(literal_edits("mv source destination", &root).is_none());
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
        assert!(
            literal_edits("mv dest renamed", &root).is_none(),
            "source contains a symbolic link"
        );
    }
}
