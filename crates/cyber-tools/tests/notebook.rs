mod support;

use serde_json::{Value, json};
use support::{Fixture, failed, ok};

fn notebook() -> Value {
    json!({"nbformat":4,"nbformat_minor":5,"metadata":{"kernelspec":{"name":"python3","display_name":"Python"},"custom":{"nested":[1,2]}},"vendor":"preserved","cells":[
        {"id":"code_1","cell_type":"code","metadata":{"tags":["keep"]},"source":["print('old')\n"],"execution_count":7,"outputs":[{"output_type":"stream","name":"stdout","text":["old\n"]}]},
        {"id":"markdown_1","cell_type":"markdown","metadata":{"custom":"keep"},"source":"![image](attachment:image.png)","attachments":{"image.png":{"image/png":"preserved"}}},
        {"id":"code_2","cell_type":"code","metadata":{},"source":"print('untouched')","execution_count":8,"outputs":[{"output_type":"display_data","data":{"application/json":{"value":42}},"metadata":{"custom":true}}]}
    ]})
}

fn write(fixture: &Fixture, notebook: &Value) {
    fixture.write("work.ipynb", &serde_json::to_string(notebook).unwrap());
}

fn read(fixture: &Fixture) -> Value {
    serde_json::from_str(&fixture.read("work.ipynb")).unwrap()
}

#[tokio::test]
async fn replace_by_id_preserves_other_cells_metadata_and_clears_edited_code_output() {
    let fixture = Fixture::new();
    let before = notebook();
    write(&fixture, &before);
    let output = ok(fixture.call("accept-edits", "notebook_edit", json!({"path":"work.ipynb","mode":"replace","cell_id":"code_1","new_source":"print('λ')\nprint('next')"})).await);
    assert_eq!(output, "Edited notebook work.ipynb (replace cell 0)");
    let after = read(&fixture);
    assert_eq!(after["metadata"], before["metadata"]);
    assert_eq!(after["vendor"], before["vendor"]);
    assert_eq!(after["cells"][0]["id"], before["cells"][0]["id"]);
    assert_eq!(
        after["cells"][0]["metadata"],
        before["cells"][0]["metadata"]
    );
    assert_eq!(
        after["cells"][0]["source"],
        json!(["print('λ')\n", "print('next')"])
    );
    assert_eq!(after["cells"][0]["outputs"], json!([]));
    assert!(after["cells"][0]["execution_count"].is_null());
    assert_eq!(after["cells"][1], before["cells"][1]);
    assert_eq!(after["cells"][2], before["cells"][2]);
}

#[tokio::test]
async fn insert_markdown_at_zero_and_append_code_keep_all_existing_cells() {
    let fixture = Fixture::new();
    let before = notebook();
    write(&fixture, &before);
    ok(fixture.call("accept-edits", "notebook_edit", json!({"path":"work.ipynb","mode":"insert","cell_index":0,"cell_type":"markdown","new_source":"# Heading\n"})).await);
    let inserted = read(&fixture);
    assert_eq!(inserted["cells"][0]["cell_type"], "markdown");
    assert_eq!(inserted["cells"][0]["source"], json!(["# Heading\n"]));
    assert!(inserted["cells"][0].get("outputs").is_none());
    assert!(
        inserted["cells"][0]["id"]
            .as_str()
            .unwrap()
            .starts_with("cell_")
    );
    assert_eq!(
        inserted["cells"].as_array().unwrap()[1..],
        before["cells"].as_array().unwrap()[..]
    );
    ok(fixture
        .call(
            "accept-edits",
            "notebook_edit",
            json!({"path":"work.ipynb","mode":"insert","new_source":"x = 1"}),
        )
        .await);
    let appended = read(&fixture);
    assert_eq!(
        appended["cells"].as_array().unwrap()[..4],
        inserted["cells"].as_array().unwrap()[..]
    );
    assert_eq!(appended["cells"][4]["cell_type"], "code");
    assert_eq!(appended["cells"][4]["outputs"], json!([]));
    assert_ne!(appended["cells"][4]["id"], appended["cells"][0]["id"]);
}

#[tokio::test]
async fn delete_and_type_changes_preserve_unaffected_cells_and_source_representation() {
    let fixture = Fixture::new();
    let before = notebook();
    write(&fixture, &before);
    ok(fixture.call("accept-edits", "notebook_edit", json!({"path":"work.ipynb","mode":"replace","cell_index":1,"cell_id":"markdown_1","cell_type":"code","new_source":""})).await);
    let changed = read(&fixture);
    assert_eq!(changed["cells"][1]["source"], "");
    assert_eq!(changed["cells"][1]["outputs"], json!([]));
    assert!(changed["cells"][1].get("attachments").is_none());
    assert_eq!(
        changed["cells"][1]["metadata"],
        before["cells"][1]["metadata"]
    );
    ok(fixture.call("accept-edits", "notebook_edit", json!({"path":"work.ipynb","mode":"replace","cell_index":0,"cell_type":"raw","new_source":"raw"})).await);
    let raw = read(&fixture);
    assert!(raw["cells"][0].get("outputs").is_none());
    assert!(raw["cells"][0].get("execution_count").is_none());
    ok(fixture
        .call(
            "accept-edits",
            "notebook_edit",
            json!({"path":"work.ipynb","mode":"delete","cell_id":"markdown_1"}),
        )
        .await);
    let deleted = read(&fixture);
    assert_eq!(deleted["cells"].as_array().unwrap().len(), 2);
    assert_eq!(deleted["cells"][1], before["cells"][2]);
    assert_eq!(deleted["metadata"], before["metadata"]);
}

#[tokio::test]
async fn invalid_selectors_parameters_and_notebooks_leave_original_bytes_unchanged() {
    let fixture = Fixture::new();
    let before = notebook();
    write(&fixture, &before);
    let original = fixture.read("work.ipynb");
    let invalid = [
        json!({"mode":"replace","cell_index":0}),
        json!({"mode":"replace","new_source":"text"}),
        json!({"mode":"delete","cell_index":3}),
        json!({"mode":"insert","cell_index":4,"new_source":"text"}),
        json!({"mode":"delete","cell_id":"absent"}),
        json!({"mode":"delete","cell_index":0,"cell_id":"code_2"}),
        json!({"mode":"replace","cell_index":-1,"new_source":"text"}),
        json!({"mode":"insert","cell_type":"invalid","new_source":"text"}),
        json!({"mode":"insert","new_source":null}),
        json!({"mode":"insert","new_source":"text","cell_index":null}),
    ];
    for mut input in invalid {
        input["path"] = json!("work.ipynb");
        failed(fixture.call("accept-edits", "notebook_edit", input).await);
        assert_eq!(fixture.read("work.ipynb"), original);
    }
    for content in ["not JSON", "[]", "{\"nbformat\":3}"] {
        fixture.write("work.ipynb", content);
        failed(
            fixture
                .call(
                    "accept-edits",
                    "notebook_edit",
                    json!({"path":"work.ipynb","mode":"delete","cell_index":0}),
                )
                .await,
        );
        assert_eq!(fixture.read("work.ipynb"), content);
    }
    fixture.write("work.txt", &original);
    assert!(
        failed(
            fixture
                .call(
                    "accept-edits",
                    "notebook_edit",
                    json!({"path":"work.txt","mode":"delete","cell_index":0})
                )
                .await
        )
        .contains(".ipynb")
    );
}

#[tokio::test]
async fn invalid_or_duplicate_modern_cell_ids_refuse_without_rewriting() {
    let fixture = Fixture::new();
    let mut duplicate = notebook();
    duplicate["cells"][1]["id"] = json!("code_1");
    let mut missing = notebook();
    missing["cells"][1].as_object_mut().unwrap().remove("id");
    let mut invalid = notebook();
    invalid["cells"][1]["id"] = json!("invalid space");
    for notebook in [duplicate, missing, invalid] {
        write(&fixture, &notebook);
        let original = fixture.read("work.ipynb");
        failed(
            fixture
                .call(
                    "accept-edits",
                    "notebook_edit",
                    json!({"path":"work.ipynb","mode":"delete","cell_index":0}),
                )
                .await,
        );
        assert_eq!(fixture.read("work.ipynb"), original);
    }
}

#[tokio::test]
async fn legacy_insert_and_noop_preserve_format_version_and_original_bytes() {
    let fixture = Fixture::new();
    let mut legacy = notebook();
    legacy["nbformat_minor"] = json!(0);
    for cell in legacy["cells"].as_array_mut().unwrap() {
        cell.as_object_mut().unwrap().remove("id");
    }
    write(&fixture, &legacy);
    let original = fixture.read("work.ipynb");
    assert!(ok(fixture.call("accept-edits", "notebook_edit", json!({"path":"work.ipynb","mode":"replace","cell_index":1,"new_source":"![image](attachment:image.png)"})).await).starts_with("Notebook unchanged"));
    assert_eq!(fixture.read("work.ipynb"), original);
    ok(fixture.call("accept-edits", "notebook_edit", json!({"path":"work.ipynb","mode":"insert","cell_index":0,"new_source":"# New","cell_type":"markdown"})).await);
    let inserted = read(&fixture);
    assert_eq!(inserted["nbformat_minor"], 0);
    assert!(inserted["cells"][0].get("id").is_none());
    assert_eq!(
        inserted["cells"].as_array().unwrap()[1..],
        legacy["cells"].as_array().unwrap()[..]
    );
}

#[tokio::test]
async fn edit_permissions_and_plan_mode_are_enforced_for_both_model_catalogs() {
    let fixture = Fixture::new();
    write(&fixture, &notebook());
    let original = fixture.read("work.ipynb");
    let input = json!({"path":"work.ipynb","mode":"delete","cell_index":0});
    failed(
        fixture
            .call("default", "notebook_edit", input.clone())
            .await,
    );
    failed(fixture.call("plan", "notebook_edit", input.clone()).await);
    fixture.set_config(json!({"permissions":{"edit":"deny"}}));
    failed(fixture.call("accept-edits", "notebook_edit", input).await);
    assert_eq!(fixture.read("work.ipynb"), original);
    fixture.set_config(json!({}));
    for prefers_patch in [false, true] {
        assert!(
            fixture
                .tool_names("default", prefers_patch)
                .contains(&"notebook_edit".to_string())
        );
        assert!(
            !fixture
                .tool_names("plan", prefers_patch)
                .contains(&"notebook_edit".to_string())
        );
    }
}

#[tokio::test]
async fn noop_replacement_still_requires_edit_permission() {
    let fixture = Fixture::new();
    write(&fixture, &notebook());
    let input = json!({"path":"work.ipynb","mode":"replace","cell_id":"markdown_1","new_source":"![image](attachment:image.png)"});
    let original = fixture.read("work.ipynb");
    failed(
        fixture
            .call("default", "notebook_edit", input.clone())
            .await,
    );
    fixture.set_config(json!({"permissions":{"edit":"deny"}}));
    failed(
        fixture
            .call("accept-edits", "notebook_edit", input.clone())
            .await,
    );
    fixture.set_config(json!({"permissions":{"edit":"allow"}}));
    assert!(
        ok(fixture.call("default", "notebook_edit", input).await).starts_with("Notebook unchanged")
    );
    assert_eq!(fixture.read("work.ipynb"), original);
}

#[tokio::test]
async fn notebook_outside_location_requires_external_approval() {
    let fixture = Fixture::new();
    let outside = fixture.dir.path().join("outside.ipynb");
    let original = notebook().to_string();
    std::fs::write(&outside, &original).unwrap();
    let error = failed(
        fixture
            .call(
                "accept-edits",
                "notebook_edit",
                json!({"path":outside,"mode":"delete","cell_index":0}),
            )
            .await,
    );
    assert!(error.contains("no interactive approver"), "{error}");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), original);
}

#[tokio::test]
async fn changes_during_interactive_review_are_preserved_for_edits_and_noops() {
    use cyber_server::runtime::{PendingKind, PermissionReply};
    use support::flow::{Flow, call, text};
    for noop in [false, true] {
        let input = if noop {
            json!({"path":"work.ipynb","mode":"replace","cell_id":"markdown_1","new_source":"![image](attachment:image.png)"})
        } else {
            json!({"path":"work.ipynb","mode":"replace","cell_index":0,"new_source":"proposed"})
        };
        let flow = Flow::new(
            vec![call("cell", "notebook_edit", input), text("done")],
            true,
        );
        let before = notebook();
        write(&flow.f, &before);
        let session = flow.session("default").await;
        flow.prompt(&session, "Edit a cell").await;
        let pending = flow.pending(&session).await;
        let PendingKind::Permission(ask) = pending.kind else {
            panic!("edit permission required")
        };
        assert_eq!(ask.action, "edit");
        let mut user = before;
        user["cells"][0]["source"] = json!(["user edit"]);
        write(&flow.f, &user);
        flow.runtime
            .reply_permission(&pending.id, PermissionReply::Once)
            .await
            .unwrap();
        flow.settle(&session).await;
        assert!(
            flow.output(&session, "cell")
                .await
                .contains("changed after permission approval")
        );
        assert_eq!(read(&flow.f), user);
    }
}

#[tokio::test]
async fn auto_mode_classifies_notebook_changes_as_file_edits() {
    use cyber_server::runtime::NoSnapshots;
    use std::sync::Arc;
    use support::flow::{Flow, call, text};
    let fixture = Fixture::new();
    write(&fixture, &notebook());
    let flow = Flow::with_models(
        fixture,
        vec![
            call(
                "cell",
                "notebook_edit",
                json!({"path":"work.ipynb","mode":"replace","cell_id":"code_1","new_source":"approved"}),
            ),
            text("done"),
        ],
        false,
        Arc::new(NoSnapshots),
        vec![(
            "test/summary",
            vec![text(
                &json!({"decision":"allow","reason":"Reviewed notebook cell edit"}).to_string(),
            )],
        )],
    );
    let session = flow.session("auto").await;
    flow.prompt(&session, "Edit a notebook cell").await;
    flow.settle(&session).await;
    assert_eq!(read(&flow.f)["cells"][0]["source"], json!(["approved"]));
    assert!(
        flow.output(&session, "cell")
            .await
            .contains("Edited notebook")
    );
}

#[tokio::test]
async fn opaque_metadata_and_untouched_outputs_preserve_exact_numeric_literals() {
    for literal in ["900719925474099300000123456789", "1e9999"] {
        let fixture = Fixture::new();
        let mut before = notebook();
        before["metadata"]["precise"] = json!("EXACT_NUMBER");
        before["cells"][0]["metadata"]["precise"] = json!("EXACT_NUMBER");
        before["cells"][2]["outputs"][0]["data"]["application/json"]["value"] =
            json!("EXACT_NUMBER");
        fixture.write(
            "work.ipynb",
            &before.to_string().replace("\"EXACT_NUMBER\"", literal),
        );
        ok(fixture
            .call(
                "accept-edits",
                "notebook_edit",
                json!({"path":"work.ipynb","mode":"replace","cell_index":0,"new_source":"updated"}),
            )
            .await);
        let after = fixture.read("work.ipynb");
        assert_eq!(
            after.matches(literal).count(),
            3,
            "opaque numeric values were rewritten: {after}"
        );
        assert!(after.contains("updated"));
    }
}
