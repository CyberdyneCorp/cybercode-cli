//! Reproducible core-only 16 MiB timings. No server, terminal or provider is used.
//! Build with `cargo build -p cyber-core --bench binary_editor --profile dev`;
//! run the resulting executable directly under an OS RSS measurement tool.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native {
    use cyber_core::binary::{
        Document, Error, Offset, SearchRequest,
        draft::{ImmutableBase, PatchBatch, Splice, encode_hex},
        editor::{DraftScope, Editor, Owner, SavePreparation},
        receipts::Outcome,
        storage::StorageRoot,
    };
    use serde_json::{Value, json};
    use std::{
        cell::Cell,
        fs::File,
        io::{Seek, SeekFrom, Write},
        path::Path,
        time::Instant,
    };

    const SIZE: u64 = 16 * 1024 * 1024;
    const RUNS: usize = 5;
    const NEEDLE: &[u8] = &[255, 0, 128, 27, 254, 31, 0];

    fn ms(started: Instant) -> f64 {
        started.elapsed().as_secs_f64() * 1000.0
    }
    fn fixture(path: &Path) {
        let mut file = File::create(path).unwrap();
        let chunk: Vec<u8> = (0..65536).map(|i| (i % 251) as u8).collect();
        for _ in 0..SIZE / chunk.len() as u64 {
            file.write_all(&chunk).unwrap();
        }
        file.seek(SeekFrom::End(-(NEEDLE.len() as i64))).unwrap();
        file.write_all(NEEDLE).unwrap();
        file.sync_all().unwrap();
    }
    fn search(document: &mut Document) -> Value {
        let mut request = SearchRequest {
            revision: document.metadata().revision,
            hex: encode_hex(NEEDLE),
            offset: Offset(0),
        };
        let (started, mut longest, mut slices, mut offsets) =
            (Instant::now(), 0.0f64, 0, Vec::new());
        loop {
            let slice = Instant::now();
            let result = document.search(&request, || false).unwrap();
            longest = longest.max(ms(slice));
            slices += 1;
            offsets.extend(result.offsets);
            let Some(next) = result.next_offset else {
                break;
            };
            assert!(next.0 > request.offset.0 && slices < 1024);
            request.offset = next;
        }
        assert_eq!(offsets, vec![Offset(SIZE - NEEDLE.len() as u64)]);
        json!({"total_ms":ms(started),"longest_slice_ms":longest,"slices":slices,"match_offset":offsets[0]})
    }
    fn batch(editor: &Editor) -> PatchBatch {
        let revision = editor.draft().revision();
        let splice = |offset, delete, inserted: &[u8]| Splice {
            offset: Offset(offset),
            delete_length: Offset(delete),
            expected_hex: if delete == 0 {
                String::new()
            } else {
                encode_hex(
                    &editor
                        .draft()
                        .page(&revision, Offset(offset), delete as usize)
                        .unwrap()
                        .bytes,
                )
            },
            insert_hex: encode_hex(inserted),
        };
        PatchBatch {
            revision: revision.clone(),
            splices: vec![
                splice(1024 * 1024, 2, &[255, 128]),
                splice(8 * 1024 * 1024, 0, &[0, 255, 128]),
                splice(12 * 1024 * 1024, 4, &[]),
            ],
        }
    }
    fn commit(editor: &mut Editor, preparation: SavePreparation) {
        let operation = match preparation {
            SavePreparation::Existing(operation) => operation,
            SavePreparation::Ready(prepared) => editor.commit_save(*prepared).unwrap(),
        };
        assert_eq!(operation.outcome, Outcome::Acknowledged);
    }
    fn one(index: usize) -> Value {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let storage = StorageRoot::prepare(&root).unwrap();
        let path = project.join("bytes.bin");
        fixture(&path);
        let started = Instant::now();
        let mut document = Document::open(&project, "bytes.bin").unwrap();
        assert_eq!(
            document
                .page(&document.metadata().revision, Offset(0), 65536)
                .unwrap()
                .bytes
                .len(),
            65536
        );
        let first_page_ms = ms(started);
        let search_result = search(&mut document);
        let polls = Cell::new(0);
        let started = Instant::now();
        let result = document.search(
            &SearchRequest {
                revision: document.metadata().revision,
                hex: encode_hex(NEEDLE),
                offset: Offset(0),
            },
            || {
                polls.set(polls.get() + 1);
                polls.get() >= 3
            },
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        let cancellation_ms = ms(started);
        let owner = Owner {
            session_id: format!("ses_bench_{index}"),
            directory: project.clone(),
            root: project,
            scope: DraftScope::Manual,
        };
        let started = Instant::now();
        let mut editor = Editor::create(
            storage.open(&format!("bin_bench_{index}")).unwrap(),
            owner,
            &mut document,
            || false,
        )
        .unwrap();
        let prepare_ms = ms(started);
        let original_sha256 = editor.draft().base().sha256().to_owned();
        let patch = batch(&editor);
        let started = Instant::now();
        editor.patch("patch", &patch).unwrap();
        let patch_ms = ms(started);
        let started = Instant::now();
        editor
            .history("undo", &editor.draft().revision(), false)
            .unwrap();
        let undo_ms = ms(started);
        let started = Instant::now();
        editor
            .history("redo", &editor.draft().revision(), true)
            .unwrap();
        let redo_ms = ms(started);
        let started = Instant::now();
        let review = editor.review(0, 128, || false).unwrap();
        let review_ms = ms(started);
        assert_eq!(review.draft.proposed_length, Offset(SIZE - 1));
        let started = Instant::now();
        let prepared = editor
            .prepare_save("save", &review.fingerprint, || false)
            .unwrap();
        commit(&mut editor, prepared);
        let save_ms = ms(started);
        let mut installed = Document::open(editor.target().root.as_path(), "bytes.bin").unwrap();
        assert_eq!(
            installed.fingerprint(|| false).unwrap(),
            review.draft.proposed_sha256
        );
        let started = Instant::now();
        let review = editor.restore_review(|| false).unwrap();
        let restore_review_ms = ms(started);
        let started = Instant::now();
        let prepared = editor
            .prepare_restore("restore", &review.fingerprint, || false)
            .unwrap();
        commit(&mut editor, prepared);
        let restore_ms = ms(started);
        let mut restored = Document::open(editor.target().root.as_path(), "bytes.bin").unwrap();
        assert_eq!(restored.fingerprint(|| false).unwrap(), original_sha256);
        assert_eq!(std::fs::metadata(path).unwrap().len(), SIZE);
        json!({"first_page_ms":first_page_ms,"prepare_ms":prepare_ms,"patch_ms":patch_ms,"undo_ms":undo_ms,"redo_ms":redo_ms,"review_ms":review_ms,"save_ms":save_ms,"restore_review_ms":restore_review_ms,"restore_ms":restore_ms,"search":search_result,"cancellation_ms":cancellation_ms,"cancellation_polls":polls.get(),"restored_sha256":original_sha256})
    }
    pub fn run() {
        let runs: Vec<Value> = (0..RUNS).map(one).collect();
        let mut medians = serde_json::Map::new();
        for key in [
            "first_page_ms",
            "prepare_ms",
            "patch_ms",
            "undo_ms",
            "redo_ms",
            "review_ms",
            "save_ms",
            "restore_review_ms",
            "restore_ms",
            "cancellation_ms",
        ] {
            let mut values: Vec<_> = runs.iter().map(|run| run[key].as_f64().unwrap()).collect();
            values.sort_by(f64::total_cmp);
            medians.insert(key.into(), json!(values[RUNS / 2]));
        }
        println!("{}",serde_json::to_string_pretty(&json!({"schema":1,"fixture_bytes":Offset(SIZE),"runs":runs,"median_ms":medians,"scope":"Core engine only; excludes server, permissions, terminal rendering/input and provider inference. Fresh temporary files with warm OS executable/cache effects; no cold-cache claim."})).unwrap());
    }
}

fn main() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    native::run();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    panic!("Native binary editing benchmark requires Linux or macOS");
}
