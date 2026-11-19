use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::*;

fn write(root: &Path, rel: &str, text: &str) {
    let path = walk::absolute(root, rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn canonical(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().canonicalize().unwrap()
}

fn built(cache: &Path, root: &Path, settings: &RepoMapSettings, knobs: Knobs) -> Report {
    match build::run(cache, root, settings, knobs) {
        Ok(report) => report,
        Err(BuildError::Busy) => panic!("busy"),
        Err(BuildError::Failed(e)) => panic!("{e}"),
    }
}

fn no_hidden(_: &str) -> bool {
    false
}

fn small_repo(root: &Path) {
    write(
        root,
        "src/net/retry.rs",
        "pub struct RetryPolicy { pub max: u32 }\nimpl RetryPolicy {\n    pub fn backoff(&self) -> u32 { self.max }\n}\n",
    );
    write(
        root,
        "src/client.rs",
        "use crate::net::retry::RetryPolicy;\npub fn send(policy: &RetryPolicy) -> u32 { policy.backoff() }\n",
    );
    write(
        root,
        "src/config.rs",
        "pub struct Config { pub name: String }\npub fn load() -> Config { Config { name: String::new() } }\n",
    );
    write(root, "src/main.rs", "fn main() { let _ = load(); }\n");
    write(root, "rtl/core.sv", "module core(); endmodule\n");
}

fn query(tokens: usize) -> MapQuery {
    MapQuery {
        tokens,
        ..MapQuery::default()
    }
}

#[test]
fn second_worktree_reuses_tags() {
    let cache = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    small_repo(first.path());
    small_repo(second.path());
    let settings = RepoMapSettings::default();
    let report = built(
        cache.path(),
        &canonical(&first),
        &settings,
        Knobs::default(),
    );
    assert_eq!(report.parsed, 4, "four Rust files parsed");
    assert_eq!(report.files, 5);
    let report = built(
        cache.path(),
        &canonical(&second),
        &settings,
        Knobs::default(),
    );
    assert_eq!(report.parsed, 0, "unchanged content is not parsed again");
    assert_eq!(report.files, 5, "the new checkout has its own rows");
    let rebuilt = built(
        cache.path(),
        &canonical(&second),
        &settings,
        Knobs {
            reextract: true,
            ..Knobs::default()
        },
    );
    assert_eq!(rebuilt.parsed, 4, "--rebuild extracts every file again");
}

#[test]
fn deleted_cache_rebuilt() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    let root = canonical(&repo);
    let maps = RepoMaps::new(cache.path().to_path_buf());
    maps.build_blocking(&root, &RepoMapSettings::default(), false)
        .unwrap();
    std::fs::remove_dir_all(cache.path().join("repo-map")).unwrap();
    maps.build_blocking(&root, &RepoMapSettings::default(), false)
        .unwrap();
    let output = maps.answer(&root, &query(1024), &no_hidden).unwrap();
    assert_eq!(output.index.files, 5);
    assert_eq!(output.index.status, "complete");
}

#[test]
fn huge_monorepo_capped() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    for dir in 0..600 {
        let path = repo.path().join(format!("pkg{dir:03}"));
        std::fs::create_dir(&path).unwrap();
        for file in 0..100 {
            std::fs::write(path.join(format!("f{file:02}.txt")), "x").unwrap();
        }
    }
    let settings = RepoMapSettings {
        max_files: 1_000,
        ..RepoMapSettings::default()
    };
    let report = built(cache.path(), &canonical(&repo), &settings, Knobs::default());
    assert_eq!(report.status, "truncated");
    assert_eq!(report.files, 1_000);
    let maps = RepoMaps::new(cache.path().to_path_buf());
    let output = maps
        .answer(&canonical(&repo), &query(256), &no_hidden)
        .unwrap();
    assert!(output.tool_text().contains("(index truncated)"));
    assert!(output.files.iter().all(|f| f.path.as_str() < "pkg010"));
}

#[test]
fn two_processes_one_builder() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    let root = canonical(&repo);
    built(
        cache.path(),
        &root,
        &RepoMapSettings::default(),
        Knobs::default(),
    );
    // Another process holds the checkout's lock.
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(
            cache
                .path()
                .join("repo-map")
                .join(format!("{}.lock", store::root_id(&root))),
        )
        .unwrap();
    lock.lock().unwrap();
    assert!(matches!(
        build::run(
            cache.path(),
            &root,
            &RepoMapSettings::default(),
            Knobs::default()
        ),
        Err(BuildError::Busy)
    ));
    let maps = RepoMaps::new(cache.path().to_path_buf());
    maps.build_blocking(&root, &RepoMapSettings::default(), false)
        .unwrap();
    let output = maps.answer(&root, &query(1024), &no_hidden).unwrap();
    assert_eq!(output.index.files, 5, "readers get the committed state");
    assert_eq!(output.index.status, "stale");
    lock.unlock().unwrap();
}

#[test]
fn partial_build_resumes() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    for i in 0..1_200 {
        write(
            repo.path(),
            &format!("src/m{i:04}.rs"),
            &format!("pub fn f{i}() {{}}\n"),
        );
    }
    let root = canonical(&repo);
    let settings = RepoMapSettings::default();
    let first = built(
        cache.path(),
        &root,
        &settings,
        Knobs {
            stop_after_batches: Some(1),
            ..Knobs::default()
        },
    );
    assert_eq!(first.status, "partial");
    assert_eq!(first.files, build::BATCH);
    let second = built(cache.path(), &root, &settings, Knobs::default());
    assert_eq!(second.status, "complete");
    assert_eq!(second.files, 1_200);
    assert_eq!(second.parsed, 700, "only the files left over are parsed");
}

#[test]
fn unsupported_language_listed_by_path() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    write(
        repo.path(),
        "src/top.rs",
        "// core core core\npub fn wire() { let core = 1; let _ = core; }\n",
    );
    let root = canonical(&repo);
    let maps = RepoMaps::new(cache.path().to_path_buf());
    maps.build_blocking(&root, &RepoMapSettings::default(), false)
        .unwrap();
    let output = maps.answer(&root, &query(4096), &no_hidden).unwrap();
    let sv = output
        .files
        .iter()
        .find(|f| f.path == "rtl/core.sv")
        .unwrap();
    assert!(sv.symbols.is_empty());
    assert_eq!(output.index.languages.get("other"), Some(&1));
    assert!(
        output.text.contains("other files:\nrtl/core.sv\n")
            || output.text.contains("rtl/core.sv\n")
    );
}

#[test]
fn focus_symbol_pulls_in_its_definition() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    for i in 0..6 {
        write(
            repo.path(),
            &format!("src/noise{i}.rs"),
            &format!("pub fn noise{i}() {{}}\n"),
        );
    }
    let root = canonical(&repo);
    let maps = RepoMaps::new(cache.path().to_path_buf());
    maps.build_blocking(&root, &RepoMapSettings::default(), false)
        .unwrap();
    let mut q = query(2048);
    q.focus_symbols = vec!["RetryPolicy".into()];
    let output = maps.answer(&root, &q, &no_hidden).unwrap();
    let rank = |path: &str| output.files.iter().find(|f| f.path == path).unwrap().rank;
    for other in ["src/config.rs", "src/main.rs", "src/noise0.rs"] {
        assert!(rank("src/net/retry.rs") > rank(other), "{other}");
        assert!(rank("src/client.rs") > rank(other), "{other}");
    }
    assert!(
        output.files[0].path == "src/net/retry.rs",
        "{}",
        output.text
    );
}

#[test]
fn recent_session_files_bias_ranking() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    let root = canonical(&repo);
    let maps = RepoMaps::new(cache.path().to_path_buf());
    maps.build_blocking(&root, &RepoMapSettings::default(), false)
        .unwrap();
    let plain = maps.answer(&root, &query(2048), &no_hidden).unwrap();
    let mut q = query(2048);
    q.session_files = vec!["src/main.rs".into()];
    let biased = maps.answer(&root, &q, &no_hidden).unwrap();
    let rank = |out: &MapOutput| {
        out.files
            .iter()
            .find(|f| f.path == "src/main.rs")
            .unwrap()
            .rank
    };
    assert!(
        rank(&biased) > rank(&plain) * 1.5,
        "{} vs {}",
        rank(&biased),
        rank(&plain)
    );
}

#[test]
fn denied_files_are_left_out() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    write(
        repo.path(),
        "secrets/keys.rs",
        "pub fn api_key() -> &'static str { \"\" }\n",
    );
    let root = canonical(&repo);
    let maps = RepoMaps::new(cache.path().to_path_buf());
    maps.build_blocking(&root, &RepoMapSettings::default(), false)
        .unwrap();
    let hidden = |path: &str| path.starts_with("secrets/");
    let mut q = query(8192);
    q.focus_paths = vec!["secrets/keys.rs".into()];
    let output = maps.answer(&root, &q, &hidden).unwrap();
    assert!(!output.text.contains("secrets"), "{}", output.text);
    assert!(!output.files.iter().any(|f| f.path.starts_with("secrets")));
    assert_eq!(output.not_indexed, vec!["secrets/keys.rs".to_string()]);
}

#[test]
fn path_restricts_the_map() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    let root = canonical(&repo);
    let maps = RepoMaps::new(cache.path().to_path_buf());
    maps.build_blocking(&root, &RepoMapSettings::default(), false)
        .unwrap();
    let mut q = query(4096);
    q.path = Some("src/net".into());
    let output = maps.answer(&root, &q, &no_hidden).unwrap();
    let paths: Vec<_> = output.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["src/net/retry.rs"]);
}

#[test]
fn same_input_same_output() {
    // The tag fixtures form one small repository; its rendering is pinned by a golden
    // file checked on every CI platform.
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo_map/tags");
    let mut files: Vec<IndexedFile> = std::fs::read_dir(&fixtures)
        .unwrap()
        .filter_map(|e| {
            let path = e.unwrap().path();
            let lang = lang::Lang::of(&path)?;
            let tags = extract::extract(lang, &std::fs::read(&path).unwrap()).unwrap();
            Some(IndexedFile {
                path: path.file_name()?.to_str()?.to_owned(),
                lang: lang.id().into(),
                tags: Some(Arc::new(tags)),
                parse_timeout: false,
            })
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let q = query(1024);
    let first = compose(&files, &q, &no_hidden);
    let second = compose(&files, &q, &no_hidden);
    assert_eq!(first.text, second.text);
    let ranks: Vec<String> = first
        .files
        .iter()
        .map(|f| format!("{} {:.12}", f.path, f.rank))
        .collect();
    let actual = format!("{}---\n{}\n", first.text, ranks.join("\n"));
    let golden = fixtures.parent().unwrap().join("golden_map.txt");
    if std::env::var_os("CYBER_UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(&golden)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_edit_visible_immediately() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    write(repo.path(), "src/net.rs", "pub fn connect() {}\n");
    let root = canonical(&repo);
    let maps = Arc::new(RepoMaps::new(cache.path().to_path_buf()));
    let settings = RepoMapSettings::default();
    let mut q = query(2048);
    q.focus_paths = vec!["src/net.rs".into()];
    let first = maps
        .query(&root, &settings, &q, Arc::new(no_hidden), TOOL_WAIT)
        .await
        .unwrap();
    assert!(!first.text.contains("retry_policy"));
    write(
        repo.path(),
        "src/net.rs",
        "pub fn connect() {}\npub fn retry_policy() -> u32 { 3 }\n",
    );
    maps.mark_dirty(&root.join("src/net.rs"));
    let second = maps
        .query(&root, &settings, &q, Arc::new(no_hidden), TOOL_WAIT)
        .await
        .unwrap();
    assert!(
        second.text.contains("pub fn retry_policy() -> u32"),
        "{}",
        second.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_when_stat_walk_exceeds_bound() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    let root = canonical(&repo);
    let maps = Arc::new(RepoMaps::new(cache.path().to_path_buf()));
    let settings = RepoMapSettings::default();
    maps.build_blocking(&root, &settings, false).unwrap();
    maps.set_knobs(Knobs {
        walk_delay: Duration::from_millis(800),
        ..Knobs::default()
    });
    maps.mark_dirty(&root.join("src/main.rs"));
    let started = std::time::Instant::now();
    let output = maps
        .query(
            &root,
            &settings,
            &query(1024),
            Arc::new(no_hidden),
            TOOL_WAIT,
        )
        .await
        .unwrap();
    assert_eq!(output.index.status, "stale");
    assert_eq!(output.index.files, 5, "the last snapshot answers");
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_call_during_the_initial_build() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    let root = canonical(&repo);
    let maps = Arc::new(RepoMaps::new(cache.path().to_path_buf()));
    maps.set_knobs(Knobs {
        walk_delay: Duration::from_millis(300),
        ..Knobs::default()
    });
    let settings = RepoMapSettings::default();
    let output = maps
        .query(
            &root,
            &settings,
            &query(1024),
            Arc::new(no_hidden),
            Duration::from_millis(200),
        )
        .await
        .unwrap();
    assert_eq!(output.index.status, "building");
    assert_eq!(output.tool_text(), "No indexed files");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_git_location_builds_on_demand() {
    let cache = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    small_repo(repo.path());
    let root = canonical(&repo);
    assert!(!root.join(".git").exists());
    let maps = Arc::new(RepoMaps::new(cache.path().to_path_buf()));
    let output = maps
        .query(
            &root,
            &RepoMapSettings::default(),
            &query(1024),
            Arc::new(no_hidden),
            TOOL_WAIT,
        )
        .await
        .unwrap();
    assert_eq!(output.index.status, "complete");
    assert!(output.tool_text().starts_with("Repository map: "));
}

#[test]
fn home_and_root_are_refused_scopes() {
    let home = Path::new("/home/user");
    assert!(refused_scope(home, home));
    assert!(refused_scope(Path::new("/"), home));
    assert!(!refused_scope(Path::new("/home/user/repo"), home));
}
