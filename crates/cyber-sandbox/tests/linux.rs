//! Bubblewrap enforcement, exercised with real commands.
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyber_sandbox::proxy::{Decide, Endpoint, Proxy};
use cyber_sandbox::{Launch, SandboxConfig, matches_domain, protected_in, wrap};
use serde_json::json;

struct Setup {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
    home: PathBuf,
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let (root, outside, home) = (base.join("repo"), base.join("outside"), base.join("home"));
    for dir in [
        root.join(".git"),
        root.join(".cyber/plans"),
        outside.clone(),
        home.join(".ssh"),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(root.join(".cyber/cyber.jsonc"), "{}").unwrap();
    std::fs::write(home.join(".ssh/id_ed25519"), "PRIVATE").unwrap();
    std::fs::write(home.join(".netrc"), "machine x password y").unwrap();
    std::fs::write(home.join("notes.txt"), "public").unwrap();
    Setup {
        _tmp: tmp,
        root,
        outside,
        home,
    }
}

fn launch(s: &Setup, config: serde_json::Value, proxy: Option<Endpoint>) -> Launch {
    let config = SandboxConfig::resolve(&config, &BTreeMap::new(), None, &s.home);
    Launch {
        unreadable: config.unreadable(&s.home),
        writable: vec![s.root.clone()],
        read_only: protected_in(&s.root),
        proxy,
        helper: Some(PathBuf::from(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))),
        config,
    }
}

async fn run(l: &Launch, script: &str) -> (bool, String) {
    let w = wrap(l, "/bin/sh", &["-c".to_string(), script.to_string()]).unwrap();
    let out = tokio::process::Command::new(&w.program)
        .args(&w.args)
        .envs(&w.env)
        .output()
        .await
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn q(p: &Path) -> String {
    format!("'{}'", p.display())
}

#[tokio::test]
async fn workspace_write_confines_writes_to_the_roots() {
    let s = setup();
    let l = launch(&s, json!({}), None);
    let (ok, out) = run(&l, &format!("echo ok > {}", q(&s.root.join("a.txt")))).await;
    assert!(ok, "{out}");
    assert!(
        run(
            &l,
            &format!("echo plan > {}", q(&s.root.join(".cyber/plans/p.md")))
        )
        .await
        .0
    );
    assert!(
        !run(&l, &format!("echo no > {}", q(&s.outside.join("x.txt"))))
            .await
            .0
    );
    assert!(
        !run(&l, &format!("echo no > {}", q(&s.root.join(".git/config"))))
            .await
            .0
    );
    assert!(
        !run(
            &l,
            &format!("echo no > {}", q(&s.root.join(".cyber/cyber.jsonc")))
        )
        .await
        .0
    );
    assert!(!s.outside.join("x.txt").exists());
    assert_eq!(
        std::fs::read_to_string(s.root.join(".cyber/cyber.jsonc")).unwrap(),
        "{}"
    );
}

#[tokio::test]
async fn read_only_allows_no_writes_and_full_access_is_unconfined() {
    let s = setup();
    let ro = launch(&s, json!({"sandbox": {"policy": "read-only"}}), None);
    assert!(
        !run(&ro, &format!("echo no > {}", q(&s.root.join("a.txt"))))
            .await
            .0
    );
    assert!(
        run(&ro, &format!("cat {}", q(&s.home.join("notes.txt"))))
            .await
            .0
    );
    let full = launch(&s, json!({"sandbox": {"policy": "full-access"}}), None);
    assert!(
        run(
            &full,
            &format!("echo yes > {}", q(&s.outside.join("y.txt")))
        )
        .await
        .0
    );
}

#[tokio::test]
async fn credential_files_are_unreadable() {
    let s = setup();
    let l = launch(&s, json!({}), None);
    for secret in [s.home.join(".ssh/id_ed25519"), s.home.join(".netrc")] {
        let (_, out) = run(&l, &format!("cat {} 2>&1", q(&secret))).await;
        assert!(
            !out.contains("PRIVATE") && !out.contains("password"),
            "{out}"
        );
    }
    assert!(
        run(&l, &format!("cat {}", q(&s.home.join("notes.txt"))))
            .await
            .0
    );
}

#[tokio::test]
async fn everyday_tools_still_work() {
    let s = setup();
    let l = launch(&s, json!({}), None);
    for cmd in [
        "git --version",
        "ls /",
        "python3 -c 'print(1)'",
        "date",
        "uname -a",
    ] {
        let (ok, out) = run(&l, &format!("cd {} && {cmd}", q(&s.root))).await;
        assert!(ok, "{cmd}: {out}");
    }
}

async fn origin() -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                    )
                    .await;
            });
        }
    });
    port
}

#[tokio::test]
async fn network_goes_only_through_the_allowlist_proxy() {
    let s = setup();
    let port = origin().await;
    let allowed = vec!["localhost".to_string()];
    let decide: Decide = Arc::new(move |host: String| {
        let ok = matches_domain(&host, &allowed);
        Box::pin(async move { ok })
    });
    let proxy = Proxy::start_unix(decide, &s.root.join("proxy.sock"))
        .await
        .unwrap();
    let l = launch(&s, json!({}), Some(proxy.endpoint.clone()));
    let (ok, out) = run(
        &l,
        &format!("curl -s --max-time 5 http://localhost:{port}/"),
    )
    .await;
    assert!(
        ok && out.contains("hello"),
        "allowed through the proxy: {out}"
    );
    let (_, out) = run(
        &l,
        &format!("curl -s --max-time 5 http://127.0.0.1:{port}/"),
    )
    .await;
    assert!(
        out.contains("Blocked by the Cyber sandbox: 127.0.0.1"),
        "blocked host: {out}"
    );
    let (ok, _) = run(
        &l,
        &format!("curl -s --noproxy '*' --max-time 5 http://127.0.0.1:{port}/"),
    )
    .await;
    assert!(
        !ok,
        "the private network namespace has no route to the host"
    );
}

#[tokio::test]
async fn network_off_blocks_everything() {
    let s = setup();
    let port = origin().await;
    let l = launch(&s, json!({"sandbox": {"network": "off"}}), None);
    let (ok, _) = run(
        &l,
        &format!("curl -s --max-time 5 http://127.0.0.1:{port}/"),
    )
    .await;
    assert!(!ok);
}
