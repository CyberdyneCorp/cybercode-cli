//! The bundled languages of the repository map: grammar, tag query and file extensions
//! (`code-intelligence` → Repository map symbol extraction).

use std::path::Path;
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

/// Bumped when the extractor's own logic changes, so stored tags are extracted again.
const EXTRACTOR_REVISION: &str = "2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    C,
    Cpp,
    Java,
    Solidity,
    Bash,
}

pub const ALL: [Lang; 11] = [
    Lang::Rust,
    Lang::Python,
    Lang::JavaScript,
    Lang::TypeScript,
    Lang::Tsx,
    Lang::Go,
    Lang::C,
    Lang::Cpp,
    Lang::Java,
    Lang::Solidity,
    Lang::Bash,
];

/// The language label of a file without a bundled grammar.
pub const OTHER: &str = "other";

impl Lang {
    pub fn id(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Go => "go",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Java => "java",
            Self::Solidity => "solidity",
            Self::Bash => "bash",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        ALL.into_iter().find(|lang| lang.id() == id)
    }

    /// The bundled language of `path`, by extension.
    pub fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => Self::Rust,
            "py" | "pyi" => Self::Python,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "go" => Self::Go,
            "c" | "h" => Self::C,
            "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" => Self::Cpp,
            "java" => Self::Java,
            "sol" => Self::Solidity,
            "sh" | "bash" => Self::Bash,
            _ => return None,
        })
    }

    pub fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::Solidity => tree_sitter_solidity::LANGUAGE.into(),
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
        }
    }

    pub fn query_source(self) -> &'static str {
        match self {
            Self::Rust => include_str!("queries/rust.scm"),
            Self::Python => include_str!("queries/python.scm"),
            Self::JavaScript => include_str!("queries/javascript.scm"),
            Self::TypeScript => include_str!("queries/typescript.scm"),
            Self::Tsx => concat!(
                include_str!("queries/typescript.scm"),
                include_str!("queries/tsx.scm")
            ),
            Self::Go => include_str!("queries/go.scm"),
            Self::C => include_str!("queries/c.scm"),
            Self::Cpp => include_str!("queries/cpp.scm"),
            Self::Java => include_str!("queries/java.scm"),
            Self::Solidity => include_str!("queries/solidity.scm"),
            Self::Bash => include_str!("queries/bash.scm"),
        }
    }

    /// The pinned grammar crate version (see `Cargo.toml`).
    fn grammar_version(self) -> &'static str {
        match self {
            Self::Rust => "tree-sitter-rust 0.24.2",
            Self::Python => "tree-sitter-python 0.25",
            Self::JavaScript => "tree-sitter-javascript 0.25",
            Self::TypeScript | Self::Tsx => "tree-sitter-typescript 0.23.2",
            Self::Go => "tree-sitter-go 0.25.0",
            Self::C => "tree-sitter-c 0.24.2",
            Self::Cpp => "tree-sitter-cpp 0.23.4",
            Self::Java => "tree-sitter-java 0.23.5",
            Self::Solidity => "tree-sitter-solidity 1.2.13",
            Self::Bash => "tree-sitter-bash 0.25",
        }
    }

    /// Hash of the query text, the grammar version and the extractor revision: stored
    /// tags of another version are extracted again.
    pub fn extractor_version(self) -> &'static str {
        static VERSIONS: OnceLock<Vec<String>> = OnceLock::new();
        let versions = VERSIONS.get_or_init(|| {
            ALL.iter()
                .map(|lang| {
                    let mut hash = Sha256::new();
                    hash.update(EXTRACTOR_REVISION);
                    hash.update(lang.grammar_version());
                    hash.update(lang.query_source());
                    hex(&hash.finalize()[..8])
                })
                .collect()
        });
        &versions[self as usize]
    }

    /// The compiled tag query. A query that fails to compile leaves the language
    /// path-only rather than failing the build.
    pub fn query(self) -> Option<&'static tree_sitter::Query> {
        static QUERIES: OnceLock<Vec<Option<tree_sitter::Query>>> = OnceLock::new();
        let queries = QUERIES.get_or_init(|| {
            ALL.iter()
                .map(|lang| tree_sitter::Query::new(&lang.grammar(), lang.query_source()).ok())
                .collect()
        });
        queries[self as usize].as_ref()
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_query_compiles_against_its_grammar() {
        for lang in ALL {
            if let Err(error) = tree_sitter::Query::new(&lang.grammar(), lang.query_source()) {
                panic!("{} query: {error}", lang.id());
            }
        }
    }

    #[test]
    fn extensions_map_to_languages() {
        assert_eq!(Lang::of(Path::new("a/b.rs")), Some(Lang::Rust));
        assert_eq!(Lang::of(Path::new("x.TSX")), Some(Lang::Tsx));
        assert_eq!(Lang::of(Path::new("rtl/core.sv")), None);
        for lang in ALL {
            assert_eq!(Lang::from_id(lang.id()), Some(lang));
        }
    }
}
