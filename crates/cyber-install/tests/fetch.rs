//! Release downloads (`installation-upgrade` → Self-update integrity) against a loopback
//! server: an interrupted or oversize body is an error and leaves no file behind.

use std::path::Path;
use std::time::Duration;

use cyber_install::fetch::{FetchError, Fetcher};

mod support {
    pub mod stub_release;
}
use support::stub_release::{Reply, StubRelease};

fn files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn fetcher() -> Fetcher {
    Fetcher::new(Duration::from_secs(5)).unwrap()
}

#[tokio::test]
async fn an_archive_streams_to_a_hidden_file_beside_the_target() {
    let stub = StubRelease::start();
    stub.route("/a.tar.gz", vec![Reply::Ok(vec![7; 300_000])]);
    let dir = tempfile::tempdir().unwrap();
    let path = fetcher()
        .to_file(&format!("{}/a.tar.gz", stub.base_url), dir.path())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), vec![7; 300_000]);
    assert_eq!(path.parent(), Some(dir.path()));
    assert!(files(dir.path())[0].starts_with(".cyber-upgrade-"));
    drop(path);
    assert!(files(dir.path()).is_empty());
}

#[tokio::test]
async fn a_truncated_body_is_an_error_and_leaves_nothing() {
    let stub = StubRelease::start();
    let body = vec![1; 1000];
    stub.route(
        "/a.tar.gz",
        vec![Reply::Truncated {
            declared: 5000,
            body: body.clone(),
        }],
    );
    stub.route(
        "/SHA256SUMS",
        vec![Reply::Truncated {
            declared: 5000,
            body,
        }],
    );
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/a.tar.gz", stub.base_url);
    let err = fetcher().to_file(&url, dir.path()).await.unwrap_err();
    assert!(matches!(err, FetchError::Http { .. }), "{err}");
    assert!(files(dir.path()).is_empty(), "{:?}", files(dir.path()));
    let err = fetcher()
        .bytes(&format!("{}/SHA256SUMS", stub.base_url))
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Http { .. }), "{err}");
}

#[tokio::test]
async fn an_oversize_body_is_refused() {
    let stub = StubRelease::start();
    stub.route("/a.tar.gz", vec![Reply::Ok(vec![0; 4096])]);
    stub.route("/SHA256SUMS", vec![Reply::Ok(vec![b'a'; 2 * 1024 * 1024])]);
    let dir = tempfile::tempdir().unwrap();
    let mut small = fetcher();
    small.max_archive_bytes = 4095;
    let url = format!("{}/a.tar.gz", stub.base_url);
    let err = small.to_file(&url, dir.path()).await.unwrap_err();
    assert!(
        matches!(err, FetchError::TooLarge { max: 4095, .. }),
        "{err}"
    );
    assert!(files(dir.path()).is_empty());
    let err = fetcher()
        .bytes(&format!("{}/SHA256SUMS", stub.base_url))
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::TooLarge { .. }), "{err}");
}

#[tokio::test]
async fn http_errors_are_not_written() {
    let stub = StubRelease::start();
    let dir = tempfile::tempdir().unwrap();
    let err = fetcher()
        .to_file(&format!("{}/missing", stub.base_url), dir.path())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("HTTP 404"), "{err}");
    assert!(files(dir.path()).is_empty());
}
