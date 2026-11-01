use super::*;
use std::io::{Seek, SeekFrom, Write};

fn fixture(bytes: &[u8]) -> (tempfile::TempDir, Document) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bytes"), bytes).unwrap();
    let root = dir.path().canonicalize().unwrap();
    let doc = Document::open(&root, "bytes").unwrap();
    (dir, doc)
}

#[test]
fn offsets_are_exact_strings_and_never_json_numbers() {
    for n in [0, 1, (1u64 << 53) + 1, u64::MAX] {
        let value = serde_json::to_string(&Offset(n)).unwrap();
        assert_eq!(value, format!("\"{n}\""));
        assert_eq!(serde_json::from_str::<Offset>(&value).unwrap(), Offset(n));
    }
    for value in [
        "1",
        "-1",
        "\"01\"",
        "\"+1\"",
        "\"0x10\"",
        "\"18446744073709551616\"",
        "\"\"",
        "\" 1\"",
    ] {
        assert!(serde_json::from_str::<Offset>(value).is_err(), "{value}");
    }
    assert_eq!(schemars::schema_for!(Offset).get("type").unwrap(), "string");
}

#[test]
fn pages_preserve_every_byte_without_decoding_and_enforce_bounds() {
    let bytes: Vec<u8> = (0..=255).collect();
    let (_dir, mut doc) = fixture(&bytes);
    let revision = doc.metadata().revision;
    let page = doc.page(&revision, Offset(0), DEFAULT_PAGE).unwrap();
    assert_eq!(page.bytes, bytes);
    assert!(page.eof);
    assert_eq!(page.sha256.len(), 64);
    assert!(
        doc.page(&revision, Offset(256), 1)
            .unwrap()
            .bytes
            .is_empty()
    );
    for (offset, limit) in [(0, 0), (0, MAX_PAGE + 1), (257, 1), (u64::MAX, 1)] {
        assert!(doc.page(&revision, Offset(offset), limit).is_err());
    }
    assert!(doc.page("other", Offset(0), 1).is_err());
    let (_empty, mut doc) = fixture(&[]);
    assert!(
        doc.page(&doc.metadata().revision, Offset(0), 1)
            .unwrap()
            .eof
    );
}

#[test]
fn sparse_view_does_not_hash_or_copy_the_whole_source() {
    let (dir, _doc) = fixture(&[]);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("bytes"))
        .unwrap();
    let at = 1024 * 1024 * 1024;
    file.seek(SeekFrom::Start(at)).unwrap();
    file.write_all(&[0, 255, 128]).unwrap();
    let mut doc = Document::open(&dir.path().canonicalize().unwrap(), "bytes").unwrap();
    assert!(!doc.metadata().within_edit_size_limit);
    let page = doc.page(&doc.metadata().revision, Offset(at), 3).unwrap();
    assert_eq!(page.bytes, [0, 255, 128]);
    assert!(page.eof);
}

#[test]
fn live_content_or_path_replacement_invalidates_the_document() {
    let (dir, mut doc) = fixture(b"old");
    let revision = doc.metadata().revision;
    std::fs::write(dir.path().join("replacement"), b"new").unwrap();
    std::fs::rename(dir.path().join("replacement"), dir.path().join("bytes")).unwrap();
    assert!(matches!(
        doc.page(&revision, Offset(0), 3),
        Err(Error::Changed)
    ));
    let mut doc = Document::open(&dir.path().canonicalize().unwrap(), "bytes").unwrap();
    let revision = doc.metadata().revision;
    std::fs::write(dir.path().join("bytes"), b"longer").unwrap();
    assert!(matches!(
        doc.page(&revision, Offset(0), 3),
        Err(Error::Changed)
    ));
}

#[test]
fn search_preserves_cross_chunk_and_request_matches_once() {
    let mut bytes = vec![0u8; MAX_SEARCH_BYTES + 32];
    for at in [DEFAULT_PAGE - 2, MAX_SEARCH_BYTES - 2] {
        bytes[at..at + 4].copy_from_slice(b"abcd");
    }
    let (_dir, mut doc) = fixture(&bytes);
    let mut request = SearchRequest {
        revision: doc.metadata().revision,
        hex: "61626364".into(),
        offset: Offset(0),
    };
    let mut found = Vec::new();
    loop {
        let result = doc.search(&request, || false).unwrap();
        assert!(result.scanned.0 <= MAX_SEARCH_BYTES as u64);
        found.extend(result.offsets);
        match result.next_offset {
            Some(next) => {
                assert!(next.0 > request.offset.0);
                request.offset = next;
            }
            None => break,
        }
    }
    assert_eq!(
        found,
        [
            Offset((DEFAULT_PAGE - 2) as u64),
            Offset((MAX_SEARCH_BYTES - 2) as u64)
        ]
    );
}

#[test]
fn overlapping_search_is_bounded_and_cancellable() {
    let (_dir, mut doc) = fixture(&vec![b'a'; 500]);
    let mut request = SearchRequest {
        revision: doc.metadata().revision,
        hex: "6161".into(),
        offset: Offset(0),
    };
    let first = doc.search(&request, || false).unwrap();
    assert_eq!(first.offsets.len(), MAX_MATCHES);
    assert_eq!(first.offsets[99], Offset(99));
    request.offset = first.next_offset.unwrap();
    let second = doc.search(&request, || false).unwrap();
    assert_eq!(second.offsets[0], Offset(100));
    assert!(matches!(
        doc.search(&request, || true),
        Err(Error::Cancelled)
    ));
    for hex in [
        String::new(),
        "a".into(),
        "zz".into(),
        "00".repeat(MAX_PATTERN + 1),
    ] {
        request.hex = hex;
        assert!(doc.search(&request, || false).is_err());
    }
}

#[test]
fn nonregular_traversal_and_parent_replacement_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/bytes"), b"original").unwrap();
    for path in ["", ".", "..", "/etc/passwd", "nested"] {
        assert!(Document::open(&root, path).is_err(), "{path}");
    }
    let mut doc = Document::open(&root, "nested/bytes").unwrap();
    std::fs::rename(root.join("nested"), root.join("old")).unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/bytes"), b"replaced").unwrap();
    assert!(matches!(
        doc.page(&doc.metadata().revision, Offset(0), 8),
        Err(Error::Changed)
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("old/bytes"), root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("old"), root.join("parent-link")).unwrap();
        let mut linked = Document::open(&root, "link").unwrap();
        assert_eq!(linked.metadata().path, "old/bytes");
        assert_eq!(
            linked
                .page(&linked.metadata().revision, Offset(0), 8)
                .unwrap()
                .bytes,
            b"original"
        );
        assert!(Document::open(&root, "parent-link/bytes").is_ok());
        std::fs::remove_file(root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("nested/bytes"), root.join("link")).unwrap();
        assert!(matches!(
            linked.page(&linked.metadata().revision, Offset(0), 8),
            Err(Error::Changed)
        ));
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"never read").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), root.join("outside")).unwrap();
        assert!(Document::open(&root, "outside").is_err());
        assert!(Document::open(&root, "/dev/zero").is_err());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn native_mac_sparse_view_and_search_preserve_an_odd_offset_above_two_to_the_53() {
    let (dir, _doc) = fixture(&[]);
    let at = (1u64 << 53) + 1;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("bytes"))
        .unwrap();
    file.seek(SeekFrom::Start(at)).unwrap();
    file.write_all(&[0, 255, 128, 27]).unwrap();
    let mut doc = Document::open(&dir.path().canonicalize().unwrap(), "bytes").unwrap();
    let revision = doc.metadata().revision;
    let page = doc.page(&revision, Offset(at), 4).unwrap();
    assert_eq!(page.offset, Offset(at));
    assert_eq!(page.bytes, [0, 255, 128, 27]);
    assert!(page.eof);
    assert_eq!(
        serde_json::to_value(page.offset).unwrap(),
        "9007199254740993"
    );
    let found = doc
        .search(
            &SearchRequest {
                revision,
                hex: "00ff".into(),
                offset: Offset(at),
            },
            || false,
        )
        .unwrap();
    assert_eq!(found.offsets, [Offset(at)]);
    assert!(found.next_offset.is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn a_zero_length_virtual_file_cannot_return_bytes_beyond_its_observed_range() {
    // procfs labels this as regular but reports length zero for its dynamic text.
    // Binary pages obey their declared length instead of turning into stream reads.
    let root = std::path::Path::new("/proc/sys/kernel")
        .canonicalize()
        .unwrap();
    let mut doc = Document::open(&root, "ostype").unwrap();
    assert_eq!(doc.metadata().length, Offset(0));
    let page = doc
        .page(&doc.metadata().revision, Offset(0), DEFAULT_PAGE)
        .unwrap();
    assert!(page.bytes.is_empty());
    assert!(page.eof);
}
