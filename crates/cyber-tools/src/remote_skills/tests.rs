use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const SOURCE: &str = "https://skills.example.test/team";

#[derive(Default)]
struct MapFetcher {
    site: Mutex<HashMap<String, Vec<u8>>>,
    failing: AtomicBool,
    calls: AtomicUsize,
}

impl MapFetcher {
    fn serve(&self, path: &str, body: &[u8]) {
        self.site
            .lock()
            .unwrap()
            .insert(format!("{SOURCE}/{path}"), body.to_vec());
    }

    fn skill(&self, description: &str) {
        self.serve(
            "index.json",
            br#"{"skills":[{"name":"lint","files":["SKILL.md","docs/a.md"]}]}"#,
        );
        self.serve(
            "lint/SKILL.md",
            format!("---\nname: lint\ndescription: {description}\n---\nBody\n").as_bytes(),
        );
        self.serve("lint/docs/a.md", b"A");
    }
}

impl RemoteSkillFetcher for MapFetcher {
    fn fetch(&self, request: FetchRequest) -> BoxFuture<'_, Result<Vec<u8>, String>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.failing.load(Ordering::SeqCst) {
                return Err("network is unreachable".into());
            }
            self.site
                .lock()
                .unwrap()
                .get(request.url.as_str())
                .cloned()
                .ok_or_else(|| format!("404 {}", request.url))
        })
    }
}

fn network() -> Network {
    Network {
        mode: NetworkMode::On,
        allowed_domains: Vec::new(),
    }
}

fn setup() -> (tempfile::TempDir, Arc<MapFetcher>, RemoteSkills) {
    let temp = tempfile::tempdir().unwrap();
    let fetcher = Arc::new(MapFetcher::default());
    let remote = RemoteSkills::new(
        temp.path().join("skills"),
        Arc::clone(&fetcher) as Arc<dyn RemoteSkillFetcher>,
    );
    (temp, fetcher, remote)
}

fn age_marker(remote: &RemoteSkills, hours: u64) {
    let path = remote.marker_path(&parse_source_url(SOURCE).unwrap());
    let mut marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    marker["fetched_at"] = (marker["fetched_at"].as_u64().unwrap() - hours * 3600).into();
    std::fs::write(&path, marker.to_string()).unwrap();
}

#[test]
fn index_rejects_escaping_and_platform_unsafe_paths() {
    for path in [
        "../evil",
        "a/../../b",
        "/etc/passwd",
        "a\\b",
        "C:evil",
        "C:/x",
        "CON",
        "docs/nul.txt",
        "com1",
        "trail.",
        ".hidden",
        "a//b",
        "a b",
        "a?b",
        "a%2e%2e",
    ] {
        assert!(validate_file_path(path).is_err(), "{path}");
        let index = format!(
            r#"{{"skills":[{{"name":"x","files":["SKILL.md","{}"]}}]}}"#,
            path.replace('\\', "\\\\")
        );
        assert!(parse_index(index.as_bytes()).is_err(), "{path}");
    }
    for path in [
        "SKILL.md",
        "docs/guide.md",
        "scripts/run-1.sh",
        "console.md",
    ] {
        validate_file_path(path).unwrap();
    }
    for index in [
        r#"{"skills":[{"name":"Bad Name","files":["SKILL.md"]}]}"#,
        r#"{"skills":[{"name":"x","files":["README.md"]}]}"#,
        r#"{"skills":[{"name":"x","files":["SKILL.md","a.md","A.md"]}]}"#,
        r#"{"skills":[{"name":"x","files":["SKILL.md"]},{"name":"x","files":["SKILL.md"]}]}"#,
    ] {
        assert!(parse_index(index.as_bytes()).is_err(), "{index}");
    }
    assert!(parse_index(&vec![b' '; INDEX_LIMIT + 1]).is_err());
    let index =
        parse_index(br#"{"skills":[{"name":"x","files":["SKILL.md"],"version":"2"}]}"#).unwrap();
    assert_eq!(index[0].version.as_deref(), Some("2"));
    for url in [
        "http://skills.example.test/team",
        "https://user:pw@skills.example.test/",
        "https://skills.example.test/?q=1",
        "file:///etc",
    ] {
        assert!(parse_source_url(url).is_err(), "{url}");
    }
}

#[tokio::test]
async fn oversized_files_fail_and_keep_the_previous_cache() {
    let (_temp, fetcher, remote) = setup();
    fetcher.skill("First");
    remote
        .refresh(&parse_source_url(SOURCE).unwrap(), &network())
        .await
        .unwrap();
    fetcher.serve("lint/docs/a.md", &vec![b'a'; FILE_LIMIT + 1]);
    let error = remote
        .refresh(&parse_source_url(SOURCE).unwrap(), &network())
        .await
        .unwrap_err();
    assert!(error.contains("size limit"), "{error}");
    let found = remote.resolve(SOURCE, network());
    assert_eq!(found.dirs.len(), 1);
    assert_eq!(
        std::fs::read(found.dirs[0].join("docs/a.md")).unwrap(),
        b"A"
    );
}

#[tokio::test]
async fn policy_fetcher_refuses_plain_http_before_contact() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/index.json", listener.local_addr().unwrap());
    let error = PolicyFetcher
        .fetch(FetchRequest {
            url: reqwest::Url::parse(&url).unwrap(),
            max_bytes: 1024,
            network: NetworkMode::On,
            allowed_domains: Vec::new(),
        })
        .await
        .unwrap_err();
    assert!(error.contains("https://"), "{error}");
    let accepted = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
    assert!(accepted.is_err(), "plain HTTP source was contacted");
}

/// One HTTP response per connection; returns the address and the request count.
async fn loopback(response: &'static str) -> (String, Arc<AtomicUsize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            seen.fetch_add(1, Ordering::SeqCst);
            let mut buffer = [0; 4096];
            let _ = socket.read(&mut buffer).await;
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (address, count)
}

#[tokio::test]
async fn policy_client_does_not_follow_redirects() {
    let (target, target_hits) =
        loopback("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}").await;
    let redirect: &'static str = Box::leak(
        format!(
            "HTTP/1.1 302 Found\r\nlocation: http://{target}/index.json\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        )
        .into_boxed_str(),
    );
    let (source, source_hits) = loopback(redirect).await;
    let error = get_bounded(&FetchRequest {
        url: reqwest::Url::parse(&format!("http://{source}/index.json")).unwrap(),
        max_bytes: 1024,
        network: NetworkMode::On,
        allowed_domains: Vec::new(),
    })
    .await
    .unwrap_err();
    assert!(error.contains("302"), "{error}");
    assert_eq!(source_hits.load(Ordering::SeqCst), 1);
    assert_eq!(target_hits.load(Ordering::SeqCst), 0);
    // The network mode applies before any contact.
    let error = get_bounded(&FetchRequest {
        url: reqwest::Url::parse(&format!("http://{source}/index.json")).unwrap(),
        max_bytes: 1024,
        network: NetworkMode::Off,
        allowed_domains: Vec::new(),
    })
    .await
    .unwrap_err();
    assert!(error.contains("network is disabled"), "{error}");
    assert_eq!(source_hits.load(Ordering::SeqCst), 1);
    // So does the allowlist: a host outside it is refused before contact.
    let error = get_bounded(&FetchRequest {
        url: reqwest::Url::parse(&format!("http://{source}/index.json")).unwrap(),
        max_bytes: 1024,
        network: NetworkMode::Proxy,
        allowed_domains: vec!["skills.example.test".into()],
    })
    .await
    .unwrap_err();
    assert!(error.contains("outside the network allowlist"), "{error}");
    assert_eq!(source_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_fresh_cache_makes_no_fetch() {
    let (_temp, fetcher, remote) = setup();
    fetcher.skill("Lint");
    let first = remote.resolve(SOURCE, network());
    assert!(first.refreshing);
    assert!(first.dirs.is_empty());
    assert!(
        first.notes[0].1.contains("not cached yet"),
        "{:?}",
        first.notes
    );
    let mut found = remote.resolve(SOURCE, network());
    for _ in 0..100 {
        if !found.dirs.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        found = remote.resolve(SOURCE, network());
    }
    assert_eq!(found.dirs.len(), 1);
    assert!(found.notes.is_empty(), "{:?}", found.notes);
    assert!(!found.refreshing);
    let calls = fetcher.calls.load(Ordering::SeqCst);
    assert_eq!(calls, 3);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!remote.resolve(SOURCE, network()).refreshing);
    assert_eq!(fetcher.calls.load(Ordering::SeqCst), calls);
    let skill = cyber_core::skills::load(&found.dirs[0]).unwrap();
    assert_eq!(skill.description, "Lint");
}

#[tokio::test]
async fn stale_cache_with_failing_fetch_keeps_files_and_warns() {
    let (_temp, fetcher, remote) = setup();
    fetcher.skill("Lint");
    let url = parse_source_url(SOURCE).unwrap();
    remote.refresh(&url, &network()).await.unwrap();
    age_marker(&remote, 25);
    fetcher.failing.store(true, Ordering::SeqCst);
    let error = remote.refresh(&url, &network()).await.unwrap_err();
    assert!(error.contains("unreachable"), "{error}");
    let stale = remote.resolve(SOURCE, network());
    assert!(stale.refreshing);
    assert_eq!(stale.dirs.len(), 1);
    assert_eq!(
        std::fs::read(stale.dirs[0].join("docs/a.md")).unwrap(),
        b"A"
    );
    let note = &stale.notes[0];
    assert_eq!(note.0.as_deref(), Some("lint"));
    assert!(note.1.contains("stale cache"), "{note:?}");
    let mut found = remote.resolve(SOURCE, network());
    for _ in 0..100 {
        if found.notes.iter().any(|n| n.1.contains("refresh failed")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        found = remote.resolve(SOURCE, network());
    }
    assert!(
        found.notes[0]
            .1
            .contains("refresh failed: network is unreachable"),
        "{:?}",
        found.notes
    );
    // A failed source is not retried on every discovery.
    assert!(!found.refreshing);
    assert_eq!(found.dirs.len(), 1);
}

#[tokio::test]
async fn changed_remote_content_invalidates_the_approval() {
    let (temp, fetcher, remote) = setup();
    let url = parse_source_url(SOURCE).unwrap();
    fetcher.skill("Lint");
    remote.refresh(&url, &network()).await.unwrap();
    let approved = remote.cached_approval_digest(SOURCE).unwrap();
    let checkout = temp.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let store = cyber_core::trust::TrustStore::new(temp.path().join("trust.json"));
    store.approve_remote_skill(&checkout, &approved).unwrap();
    assert!(
        store
            .is_remote_skill_approved(&checkout, &approved)
            .unwrap()
    );
    // The same content under another URL is a different approval.
    assert_ne!(
        approval_digest(
            "https://other.example.test/team",
            &remote.resolve(SOURCE, network()).digest.unwrap()
        ),
        approved
    );
    fetcher.skill("Changed");
    remote.refresh(&url, &network()).await.unwrap();
    let changed = remote.cached_approval_digest(SOURCE).unwrap();
    assert_ne!(changed, approved);
    assert!(!store.is_remote_skill_approved(&checkout, &changed).unwrap());
    // A SKILL.md naming another skill is refused and the approved copy is kept.
    fetcher.serve(
        "lint/SKILL.md",
        b"---\nname: other\ndescription: D\n---\nB\n",
    );
    let error = remote.refresh(&url, &network()).await.unwrap_err();
    assert!(error.contains("declares the name other"), "{error}");
    assert_eq!(remote.cached_approval_digest(SOURCE).unwrap(), changed);
}

#[tokio::test]
async fn interrupted_refresh_leftovers_and_old_generations_are_swept() {
    let (_temp, fetcher, remote) = setup();
    let url = parse_source_url(SOURCE).unwrap();
    let source = remote.source_dir(&url);
    for leftover in [".stage-1-2", "g1-2"] {
        std::fs::create_dir_all(source.join(leftover).join("lint")).unwrap();
    }
    fetcher.skill("Lint");
    remote.refresh(&url, &network()).await.unwrap();
    let first = remote.resolve(SOURCE, network()).dirs;
    fetcher.skill("Changed");
    remote.refresh(&url, &network()).await.unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&source)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != ".marker.json")
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(valid_generation(&names.pop().unwrap()));
    // A path handed out before the refresh is gone rather than silently changed.
    assert!(!first[0].exists());
}

#[tokio::test]
async fn sources_on_one_host_never_share_skill_directories() {
    let (_temp, fetcher, remote) = setup();
    let other = "https://skills.example.test/attacker";
    fetcher.skill("Team lint");
    let site = |path: &str, body: &[u8]| {
        fetcher
            .site
            .lock()
            .unwrap()
            .insert(format!("{other}/{path}"), body.to_vec());
    };
    site(
        "index.json",
        br#"{"skills":[{"name":"lint","files":["SKILL.md"]}]}"#,
    );
    site(
        "lint/SKILL.md",
        b"---\nname: lint\ndescription: Attacker\n---\nEvil\n",
    );
    let team = parse_source_url(SOURCE).unwrap();
    remote.refresh(&team, &network()).await.unwrap();
    let approved = remote.cached_approval_digest(SOURCE).unwrap();
    // The same name from another source on the same host, and the same source spelled
    // with a trailing slash, each get their own directory.
    remote
        .refresh(&parse_source_url(other).unwrap(), &network())
        .await
        .unwrap();
    let found = remote.resolve(SOURCE, network());
    assert_eq!(found.approval.as_deref(), Some(approved.as_str()));
    let skill = cyber_core::skills::load(&found.dirs[0]).unwrap();
    assert_eq!(skill.description, "Team lint");
    let attacker = remote.resolve(other, network());
    assert_ne!(attacker.dirs, found.dirs);
    assert_eq!(
        cyber_core::skills::load(&attacker.dirs[0])
            .unwrap()
            .description,
        "Attacker"
    );
    assert_ne!(
        remote.source_dir(&team),
        remote.source_dir(&parse_source_url(&format!("{SOURCE}/")).unwrap())
    );
}

#[tokio::test]
async fn cached_bytes_that_differ_from_the_marker_are_refused() {
    let (_temp, fetcher, remote) = setup();
    let url = parse_source_url(SOURCE).unwrap();
    fetcher.skill("Lint");
    remote.refresh(&url, &network()).await.unwrap();
    let dirs = remote.resolve(SOURCE, network()).dirs;
    std::fs::write(
        dirs[0].join("SKILL.md"),
        "---\nname: lint\ndescription: Swapped\n---\nEvil\n",
    )
    .unwrap();
    fetcher.failing.store(true, Ordering::SeqCst);
    let found = remote.resolve(SOURCE, network());
    assert!(found.dirs.is_empty());
    assert!(found.approval.is_none());
    assert!(found.refreshing, "a mismatched cache is refetched");
    assert!(
        found.notes[0]
            .1
            .contains("does not match its recorded digest"),
        "{:?}",
        found.notes
    );
    assert!(remote.cached_approval_digest(SOURCE).is_err());
}
