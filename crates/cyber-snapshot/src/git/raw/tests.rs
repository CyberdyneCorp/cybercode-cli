//! Real Git process accounting, isolated from other tests' PATH and environment.
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn repeated_raw_capture_images_write_each_distinct_blob_once_per_capture() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    std::fs::write(
        &wrapper,
        r#"#!/bin/sh
for argument in "$@"; do
    if [ "$argument" = hash-object ]; then
        printf 'blob\n' >> "$CYBER_RAW_BLOB_CALLS"
    fi
done
exec "$CYBER_RAW_REAL_GIT" "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let real = Command::new("which").arg("git").output().unwrap();
    assert!(real.status.success());
    let real = std::str::from_utf8(&real.stdout).unwrap().trim();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "git::raw::tests::raw_blob_process_child",
            "--nocapture",
        ])
        .env("CYBER_RAW_REAL_GIT", real)
        .env("CYBER_RAW_BLOB_CALLS", temp.path().join("calls"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "subprocess fixture with isolated Git process accounting"]
async fn raw_blob_process_child() {
    for format in ["sha1", "sha256"] {
        verify_format(format).await;
    }
}

async fn verify_format(format: &str) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let status = Command::new(std::env::var_os("CYBER_RAW_REAL_GIT").unwrap())
        .args([
            "init",
            "--quiet",
            "-b",
            "main",
            &format!("--object-format={format}"),
        ])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(status.success());
    let shadow = Shadow::locate(&root.join("snapshots"), &repo).unwrap();
    shadow.ensure_init().await.unwrap();
    let mut files = raw_files();
    let calls = std::env::var_os("CYBER_RAW_BLOB_CALLS").unwrap();
    std::fs::write(&calls, "").unwrap();
    let first = shadow.write_raw_capture(&files).await.unwrap();
    assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 2);
    verify_graph(&shadow, &first, &files).await;
    assert_eq!(shadow.write_raw_capture(&files).await.unwrap(), first);
    assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 4);
    files.get_mut("nested/file_01").unwrap().bytes = b"later bytes".to_vec();
    let changed = shadow.write_raw_capture(&files).await.unwrap();
    assert_ne!(changed, first);
    assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 7);
}

fn raw_files() -> BTreeMap<String, RawImage> {
    let mut files = (0..64)
        .map(|index| {
            (
                format!("nested/file_{index:02}"),
                RawImage {
                    mode: if index == 0 { "100755" } else { "100644" },
                    bytes: b"raw\0binary\r\n".to_vec(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    files.insert(
        "literal\nlink".into(),
        RawImage {
            mode: "120000",
            bytes: b"nested/file_00".to_vec(),
        },
    );
    files.get_mut("nested/file_63").unwrap().bytes = b"nested/file_00".to_vec();
    files
}

async fn verify_graph(shadow: &Shadow, tree: &str, files: &BTreeMap<String, RawImage>) {
    let catalog = shadow.read_catalog(tree.to_owned(), &[]).await.unwrap();
    assert_eq!(catalog.len(), 65);
    assert_eq!(catalog["nested/file_00"].0, "100755");
    assert_eq!(catalog["nested/file_01"].0, "100644");
    assert_eq!(catalog["literal\nlink"].0, "120000");
    assert_eq!(catalog["nested/file_00"].1, catalog["nested/file_01"].1);
    assert_eq!(catalog["nested/file_63"].0, "100644");
    assert_eq!(catalog["nested/file_63"].1, catalog["literal\nlink"].1);
    for (path, (_, object)) in &catalog {
        let bytes = shadow
            .run_bounded(&["cat-file", "blob", object], 1024)
            .await
            .unwrap();
        assert_eq!(bytes, files[path].bytes);
        images::verify(object, &bytes).unwrap();
    }
}
