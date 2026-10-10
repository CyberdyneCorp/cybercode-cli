//! Real framed server feedback through the public tool host.
#![cfg(unix)]

mod support;

use cyber_core::paths::Paths;
use cyber_tools::lsp::{LaunchOptions, LocalLauncher, Locations};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use support::{Fixture, ok};

const SERVER: &str = r#"
import json,sys,pathlib,urllib.parse,time
versions={}
def send(value):
    body=json.dumps(value).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body)
    sys.stdout.buffer.flush()
def diagnostic(message,severity=1):
    return {'range':{'start':{'line':2,'character':3},'end':{'line':2,'character':4}},'severity':severity,'message':message}
while True:
    length=None
    while True:
        line=sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line==b'\r\n': break
        key,value=line.decode().split(':',1)
        if key.lower()=='content-length': length=int(value)
    msg=json.loads(sys.stdin.buffer.read(length)); method=msg.get('method'); p=msg.get('params',{})
    if method=='initialize':
        root=pathlib.Path(urllib.parse.unquote(urllib.parse.urlparse(p['rootUri']).path))
        send({'jsonrpc':'2.0','id':msg['id'],'result':{'capabilities':{'textDocumentSync':{'openClose':True,'change':1,'save':{'includeText':True}}}}})
    elif method in ['textDocument/didOpen','textDocument/didChange','textDocument/didClose','workspace/didChangeWatchedFiles']:
        with (root/'sync-events').open('a') as f: f.write(json.dumps({'method':method,'params':p})+'\n')
        if method in ['textDocument/didOpen','textDocument/didChange']:
            d=p['textDocument']; versions[d['uri']]=d['version']
            if sys.argv[1]=='navigation': send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':d['uri'],'version':d['version'],'diagnostics':[diagnostic('fixture diagnostic'),diagnostic('fixture warning',2)]}})
        elif method=='textDocument/didClose': versions.pop(p['textDocument']['uri'])
        elif method=='workspace/didChangeWatchedFiles':
            for change in p['changes']:
                path=pathlib.Path(urllib.parse.unquote(urllib.parse.urlparse(change['uri']).path))
                if change['type']==3: assert not path.exists()
    elif method=='textDocument/didSave':
        uri=p['textDocument']['uri']; path=pathlib.Path(urllib.parse.unquote(urllib.parse.urlparse(uri).path))
        assert p['text']==path.read_text()
        project={f.name:f.read_text() for f in root.glob('*.txt')}
        with (root/'save-events').open('a') as f: f.write(json.dumps({'uri':uri,'version':versions[uri],'text':path.read_text(),'project':project})+'\n')
        if sys.argv[1]=='silent': continue
        errors=[diagnostic('fresh <error> & detail')]+[diagnostic('error %d'%i) for i in range(21)]+[diagnostic('warning hidden',2)]
        if sys.argv[1]!='other-only': send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':uri,'version':versions[uri],'diagnostics':errors}})
        for other in sorted(root.glob('other*.txt')):
            send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':other.as_uri(),'diagnostics':[diagnostic('other error')]}})
    elif method.startswith('textDocument/') or method=='workspace/symbol':
        with (root/'rpc-events').open('a') as f: f.write(json.dumps({'method':method,'params':p})+'\n')
        if sys.argv[1]=='navigation-delay': time.sleep(0.15)
        if sys.argv[1]=='navigation-error':
            send({'jsonrpc':'2.0','id':msg['id'],'error':{'code':-32601,'message':'private-error-secret'}});continue
        uri=p.get('textDocument',{}).get('uri',(root/'file.txt').as_uri())
        r=lambda line: {'start':{'line':line,'character':0},'end':{'line':line,'character':3}}
        if method in ['textDocument/definition','textDocument/references','textDocument/hover','textDocument/implementation','textDocument/rename']: assert p['position']=={'line':0,'character':1}
        if method=='textDocument/definition': result=[{'targetUri':(root/'target.txt').as_uri(),'targetRange':r(0),'targetSelectionRange':r(0)}]
        elif method=='textDocument/references':
            assert p['context']=={'includeDeclaration':True}
            result=[{'uri':uri,'range':r(i)} for i in range(60)]+[{'uri':'https://example.invalid/file','range':r(0)},{'uri':root.parent.joinpath('outside.txt').as_uri(),'range':r(0)}]
        elif method=='textDocument/hover': result={'contents':{'kind':'markdown','value':'int <x>\n\x1b[31m'},'range':r(0)}
        elif method=='textDocument/implementation': result={'uri':uri,'range':r(1)}
        elif method=='textDocument/documentSymbol': result=[{'name':'one','kind':13,'range':r(0),'selectionRange':r(0),'children':[{'name':'two','kind':13,'range':r(1),'selectionRange':r(1)}]}]
        elif method=='workspace/symbol':
            assert p['query']=='find'
            result=[{'name':'workspace','kind':13,'location':{'uri':(root/'target.txt').as_uri(),'range':r(0)}}]
        elif method=='textDocument/rename':
            assert p['newName']=='renamed'
            result={'changes':{uri:[{'range':r(0),'newText':'renamed'}],'https://example.invalid/file':[{'range':r(0),'newText':'external'}]},'documentChanges':[{'textDocument':{'uri':uri,'version':versions[uri]},'edits':[{'range':r(0),'newText':'renamed'}]},{'textDocument':{'uri':'https://example.invalid/file','version':None},'edits':[{'range':r(0),'newText':'external'}]},{'textDocument':{'uri':(root/'target.txt').as_uri(),'version':None},'edits':[{'range':r(1),'newText':'second edit'}]},{'kind':'create','uri':(root/'created.txt').as_uri()},{'kind':'rename','oldUri':(root/'target.txt').as_uri(),'newUri':(root/'moved.txt').as_uri()},{'kind':'rename','oldUri':(root/'folder').as_uri(),'newUri':(root/'renamed-folder').as_uri()}]}
        else: result=None
        send({'jsonrpc':'2.0','id':msg['id'],'result':result})
    elif method=='shutdown': send({'jsonrpc':'2.0','id':msg['id'],'result':None})
    elif method=='exit': break
"#;

fn setup(mode: &'static str, wait: u64) -> (Fixture, Locations) {
    let fixture = Fixture::new();
    fixture.set_config(json!({"lsp":{"diagnostics_wait_ms":wait}}));
    let mut environment = HashMap::from([(
        "CYBER_HOME".into(),
        fixture.dir.path().join("cyber").display().to_string(),
    )]);
    environment.insert("PATH".into(), std::env::var("PATH").unwrap());
    let mut paths = Paths::resolve(&environment, fixture.dir.path());
    paths.tmp = fixture.dir.path().join("lsp-tmp");
    paths.ensure().unwrap();
    let mut config = json!({"lsp":{"fixture":{"command":["python3","-u","-c",SERVER,mode],"extensions":[".txt",".ipynb"]}},"sandbox":{"network":"off"}});
    for definition in cyber_core::intelligence::builtin_servers() {
        config["lsp"][definition.id] = json!({"disabled":true});
    }
    std::fs::write(paths.config.join("cyber.jsonc"), config.to_string()).unwrap();
    let home = fixture.dir.path().to_path_buf();
    let locations = Locations::new(Arc::new(move |root| {
        let launcher = LocalLauncher::new(
            root,
            LaunchOptions {
                checkout_claim: None,
                paths: paths.clone(),
                home: home.clone(),
                environment: environment.clone(),
                profile: None,
                overrides: vec![],
                flags: json!({}),
                sandbox_policy: None,
                helper: cyber_sandbox::find_helper(),
                credential_env_names: vec![],
            },
        )
        .map_err(|error| error.error)?;
        Arc::new(launcher).pool().map_err(|error| error.error)
    }));
    fixture
        .host
        .attach_lsp_locations(locations.clone())
        .unwrap();
    (fixture, locations)
}

fn assert_errors(output: &str, file: &str) {
    assert!(
        output.contains(&format!("<diagnostics file=\"{file}\">")),
        "{output}"
    );
    assert!(
        output.contains("ERROR [3:4] fresh &lt;error&gt; &amp; detail"),
        "{output}"
    );
    assert!(output.contains("… and 2 more"), "{output}");
    assert!(!output.contains("warning hidden"), "{output}");
}

#[tokio::test]
async fn formatter_finishes_before_owned_lsp_open_and_save() {
    let (fixture, locations) = setup("all", 1000);
    fixture.set_config(json!({
        "sandbox":{"network":"off"},
        "lsp":{"diagnostics_wait_ms":1000},
        "formatters":{"fixture":{"command":["/bin/sh","-c","printf 'formatted\\n' > \"$1\"","formatter","$FILE"],"extensions":[".txt"]}}
    }));
    let output = ok(fixture
        .call(
            "bypass",
            "write",
            json!({"path":"source.txt","content":"draft\n"}),
        )
        .await);
    assert_eq!(fixture.read("source.txt"), "formatted\n");
    assert!(output.contains("Formatting result:"));
    assert_errors(&output, "source.txt");
    let saves: Vec<Value> = fixture
        .read("save-events")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0]["text"], "formatted\n");
    let opens: Vec<Value> = fixture
        .read("sync-events")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|event: &Value| event["method"] == "textDocument/didOpen")
        .collect();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0]["params"]["textDocument"]["text"], "formatted\n");
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|settlement| settlement.acknowledged)
    );
}

#[tokio::test]
async fn all_four_edit_tools_append_fresh_errors_and_write_limits_new_other_files() {
    let (fixture, locations) = setup("errors", 2000);
    for i in 0..7 {
        fixture.write(&format!("other{i}.txt"), "other");
    }
    let output = ok(fixture
        .call(
            "accept-edits",
            "write",
            json!({"path":"file.txt","content":"first"}),
        )
        .await);
    assert_errors(&output, "file.txt");
    assert_eq!(
        output.matches("<diagnostics file=\"other").count(),
        5,
        "{output}"
    );
    let output = ok(fixture
        .call(
            "accept-edits",
            "edit",
            json!({"path":"file.txt","old_string":"first","new_string":"second"}),
        )
        .await);
    assert_errors(&output, "file.txt");
    assert!(!output.contains("<diagnostics file=\"other"));
    let output = ok(fixture
        .call(
            "accept-edits",
            "write",
            json!({"path":"file.txt","content":"third"}),
        )
        .await);
    assert_errors(&output, "file.txt");
    assert!(
        !output.contains("<diagnostics file=\"other"),
        "unchanged other errors: {output}"
    );
    let output=ok(fixture.call("accept-edits","apply_patch",json!({"patch":"*** Begin Patch\n*** Update File: file.txt\n-third\n+fourth\n*** Add File: final.txt\n+final\n*** End Patch"})).await);
    assert_errors(&output, "file.txt");
    assert_errors(&output, "final.txt");
    fixture.write("work.ipynb",&json!({"nbformat":4,"nbformat_minor":5,"metadata":{},"cells":[{"cell_type":"code","id":"one","metadata":{},"source":"first","execution_count":null,"outputs":[]}]}).to_string());
    let output = ok(fixture
        .call(
            "accept-edits",
            "notebook_edit",
            json!({"path":"work.ipynb","mode":"replace","cell_index":0,"new_source":"second"}),
        )
        .await);
    assert_errors(&output, "work.ipynb");
    let saves: Vec<Value> = fixture
        .read("save-events")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(saves.len(), 6);
    let changed: Vec<_> = sync_events(&fixture)
        .into_iter()
        .filter(|event| event["method"] == "workspace/didChangeWatchedFiles")
        .map(|event| event["params"]["changes"][0]["type"].as_i64().unwrap())
        .collect();
    assert_eq!(changed, vec![1, 2, 2, 2, 1, 2]);
    for save in &saves[3..5] {
        assert_eq!(save["project"]["file.txt"], "fourth\n");
        assert_eq!(save["project"]["final.txt"], "final\n");
    }
    assert!(
        saves
            .windows(2)
            .all(|pair| pair[0]["version"].as_i64() < pair[1]["version"].as_i64())
    );
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

#[tokio::test]
async fn diagnostic_timeout_preserves_success_and_zero_wait_retains_notification() {
    for wait in [0, 200] {
        let (fixture, locations) = setup("silent", wait);
        let output = ok(fixture
            .call(
                "accept-edits",
                "write",
                json!({"path":"file.txt","content":"saved"}),
            )
            .await);
        assert!(!output.contains("<diagnostics"));
        assert_eq!(fixture.read("file.txt"), "saved");
        tokio::time::timeout(Duration::from_secs(3), async {
            while !fixture.repo.join("save-events").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(fixture.read("save-events").lines().count(), 1);
        assert!(
            locations
                .close()
                .await
                .unwrap()
                .iter()
                .all(|s| s.acknowledged)
        );
    }
}

#[tokio::test]
async fn diagnostic_wait_releases_write_lock_and_cancellation_retains_saved_file() {
    use cyber_server::runtime::ToolHost;
    use tokio_util::sync::CancellationToken;
    let (fixture, locations) = setup("silent", 2000);
    let host = fixture.host.clone();
    let invocation = fixture.invocation(
        "accept-edits",
        "write",
        json!({"path":"file.txt","content":"first"}),
    );
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let first = tokio::spawn(async move { host.execute(invocation, token).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !fixture.repo.join("save-events").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        !first.is_finished(),
        "first save should still be waiting for diagnostics"
    );
    fixture.set_config(json!({"lsp":{"diagnostics_wait_ms":0}}));
    let second = tokio::time::timeout(
        Duration::from_millis(500),
        fixture.call(
            "accept-edits",
            "write",
            json!({"path":"file.txt","content":"second"}),
        ),
    )
    .await
    .expect("diagnostic waiter held the write lock");
    assert!(!ok(second).contains("<diagnostics"));
    cancel.cancel();
    tokio::time::timeout(Duration::from_millis(500), first)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.read("file.txt"), "second");
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

async fn wait_saves(fixture: &Fixture, count: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let events =
                std::fs::read_to_string(fixture.repo.join("save-events")).unwrap_or_default();
            if events.lines().count() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn rapid_identical_saves_share_the_latest_quiet_period() {
    use cyber_server::runtime::ToolHost;
    use tokio_util::sync::CancellationToken;
    let (fixture, locations) = setup("errors", 2000);
    let host = fixture.host.clone();
    let invocation = fixture.invocation(
        "accept-edits",
        "write",
        json!({"path":"file.txt","content":"same"}),
    );
    let first =
        tokio::spawn(async move { host.execute(invocation, CancellationToken::new()).await });
    wait_saves(&fixture, 1).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let second_started = tokio::time::Instant::now();
    let second = fixture.call(
        "accept-edits",
        "write",
        json!({"path":"file.txt","content":"same"}),
    );
    let first_result = async {
        let output = first.await.unwrap();
        assert!(
            second_started.elapsed() >= Duration::from_millis(145),
            "earlier waiter ignored latest save"
        );
        output
    };
    let (first, second) = tokio::join!(first_result, second);
    assert_errors(&ok(first), "file.txt");
    assert_errors(&ok(second), "file.txt");
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

#[tokio::test]
async fn superseded_save_never_reports_other_file_errors() {
    use cyber_server::runtime::ToolHost;
    use tokio_util::sync::CancellationToken;
    let (fixture, locations) = setup("other-only", 500);
    fixture.write("other0.txt", "other");
    let host = fixture.host.clone();
    let invocation = fixture.invocation(
        "accept-edits",
        "write",
        json!({"path":"file.txt","content":"first"}),
    );
    let first =
        tokio::spawn(async move { host.execute(invocation, CancellationToken::new()).await });
    wait_saves(&fixture, 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    fixture.set_config(json!({"lsp":{"diagnostics_wait_ms":0}}));
    ok(fixture
        .call(
            "accept-edits",
            "write",
            json!({"path":"file.txt","content":"second"}),
        )
        .await);
    wait_saves(&fixture, 2).await;
    let output = ok(tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .unwrap()
        .unwrap());
    assert!(
        !output.contains("<diagnostics"),
        "superseded feedback: {output}"
    );
    assert_eq!(fixture.read("file.txt"), "second");
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

#[tokio::test]
async fn external_change_discards_all_pending_feedback() {
    use cyber_server::runtime::ToolHost;
    use tokio_util::sync::CancellationToken;
    let (fixture, locations) = setup("other-only", 500);
    fixture.write("other0.txt", "other");
    let host = fixture.host.clone();
    let invocation = fixture.invocation(
        "accept-edits",
        "write",
        json!({"path":"file.txt","content":"first"}),
    );
    let first =
        tokio::spawn(async move { host.execute(invocation, CancellationToken::new()).await });
    wait_saves(&fixture, 1).await;
    fixture.write("file.txt", "user edit");
    let output = ok(tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .unwrap()
        .unwrap());
    assert!(
        !output.contains("<diagnostics"),
        "external change feedback: {output}"
    );
    assert_eq!(fixture.read("file.txt"), "user edit");
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

fn sync_events(fixture: &Fixture) -> Vec<Value> {
    fixture
        .read("sync-events")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn patch_rename_and_delete_close_old_documents_and_publish_file_events() {
    let (fixture, locations) = setup("errors", 2000);
    ok(fixture
        .call(
            "accept-edits",
            "write",
            json!({"path":"old.txt","content":"old"}),
        )
        .await);
    let output = ok(fixture.call("accept-edits", "apply_patch", json!({"patch":"*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n-old\n+new\n*** End Patch"})).await);
    assert_errors(&output, "new.txt");
    assert!(!output.contains("<diagnostics file=\"old.txt\""));
    assert!(!fixture.repo.join("old.txt").exists());
    assert_eq!(fixture.read("new.txt"), "new\n");
    ok(fixture
        .call(
            "accept-edits",
            "apply_patch",
            json!({"patch":"*** Begin Patch\n*** Delete File: new.txt\n*** End Patch"}),
        )
        .await);
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
    let events = sync_events(&fixture);
    let closes: Vec<_> = events
        .iter()
        .filter(|event| event["method"] == "textDocument/didClose")
        .collect();
    assert_eq!(closes.len(), 2, "{events:?}");
    assert!(
        closes[0]["params"]["textDocument"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("/old.txt")
    );
    assert!(
        closes[1]["params"]["textDocument"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("/new.txt")
    );
    let changes: Vec<_> = events
        .iter()
        .filter(|event| event["method"] == "workspace/didChangeWatchedFiles")
        .map(|event| event["params"]["changes"][0].clone())
        .collect();
    assert_eq!(
        changes
            .iter()
            .map(|change| change["type"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 3, 1, 3]
    );
    let old_closed = events
        .iter()
        .position(|event| event["method"] == "textDocument/didClose")
        .unwrap();
    let new_open = events
        .iter()
        .position(|event| {
            event["method"] == "textDocument/didOpen"
                && event["params"]["textDocument"]["uri"]
                    .as_str()
                    .unwrap()
                    .ends_with("/new.txt")
        })
        .unwrap();
    assert!(old_closed < new_open);
}

#[tokio::test]
async fn cold_deletion_starts_no_server_and_zero_wait_retains_warm_deletion() {
    let (fixture, locations) = setup("errors", 2000);
    fixture.write("cold.txt", "cold");
    ok(fixture
        .call(
            "accept-edits",
            "apply_patch",
            json!({"patch":"*** Begin Patch\n*** Delete File: cold.txt\n*** End Patch"}),
        )
        .await);
    assert!(locations.status(&fixture.repo).unwrap().is_empty());
    assert!(!fixture.repo.join("sync-events").exists());
    ok(fixture
        .call(
            "accept-edits",
            "write",
            json!({"path":"warm.txt","content":"warm"}),
        )
        .await);
    fixture.set_config(json!({"lsp":{"diagnostics_wait_ms":0}}));
    ok(fixture
        .call(
            "accept-edits",
            "apply_patch",
            json!({"patch":"*** Begin Patch\n*** Delete File: warm.txt\n*** End Patch"}),
        )
        .await);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let events = sync_events(&fixture);
            if events.iter().any(|event| {
                event["method"] == "workspace/didChangeWatchedFiles"
                    && event["params"]["changes"][0]["type"] == 3
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    fixture.write("unsupported.bin", "binary");
    ok(fixture
        .call(
            "accept-edits",
            "apply_patch",
            json!({"patch":"*** Begin Patch\n*** Delete File: unsupported.bin\n*** End Patch"}),
        )
        .await);
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
    let events = sync_events(&fixture);
    assert!(
        !events
            .iter()
            .any(|event| event.to_string().contains("unsupported.bin"))
    );
}

fn navigation_fixture(mode: &'static str) -> (Fixture, Locations) {
    let (fixture, locations) = setup(mode, 2000);
    fixture.set_config(json!({"permissions":{"lsp":"allow"}}));
    fixture.write("file.txt", "hello\nworld\nthird\n");
    fixture.write("target.txt", "target\nsecond\n");
    (fixture, locations)
}

#[tokio::test]
async fn navigation_operations_validate_positions_paths_previews_and_result_limit() {
    let (fixture, locations) = navigation_fixture("navigation");
    for operation in [
        "definition",
        "references",
        "hover",
        "document_symbols",
        "workspace_symbols",
        "implementation",
    ] {
        let input = match operation {
            "workspace_symbols" => json!({"operation":operation,"query":"find"}),
            "document_symbols" => json!({"operation":operation,"path":"file.txt"}),
            _ => json!({"operation":operation,"path":"file.txt","line":1,"character":2}),
        };
        let output = ok(fixture.call("default", "lsp", input).await);
        assert!(!output.contains('\u{1b}'));
        let result: Value = serde_json::from_str(&output).unwrap();
        if operation == "definition" {
            support::golden(&fixture, "lsp", &output);
        }
        assert_eq!(result["operation"], operation);
        assert_eq!(result["servers_failed"], 0);
        let rows = result["results"].as_array().unwrap();
        assert!(!rows.is_empty(), "{result}");
        assert!(rows.len() <= 50);
        assert!(
            rows.iter()
                .all(|row| !row["path"].as_str().unwrap().starts_with('/'))
        );
        match operation {
            "references" => {
                assert_eq!(rows.len(), 50);
                assert_eq!(result["omitted"], 12);
                assert_eq!(rows[0]["line"], 1);
                assert_eq!(rows[0]["character"], 1);
                assert_eq!(rows[0]["location"], "file.txt:1:1");
                assert_eq!(rows[0]["preview"], "hello");
            }
            "definition" | "workspace_symbols" => {
                assert_eq!(rows[0]["path"], "target.txt");
                assert_eq!(rows[0]["preview"], "target");
            }
            "document_symbols" => assert_eq!(rows.len(), 2),
            "hover" => assert!(rows[0]["text"].as_str().unwrap().contains("int <x>")),
            "implementation" => assert_eq!(rows[0]["preview"], "world"),
            _ => unreachable!(),
        }
    }
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
    assert!(!fixture.repo.join("save-events").exists());
}

#[tokio::test]
async fn rename_preview_returns_text_and_resource_edits_without_mutating_files() {
    let (fixture, locations) = navigation_fixture("navigation");
    fixture.write("folder/nested.txt", "preserved folder");
    let output = ok(fixture.call("plan", "lsp", json!({"operation":"rename_preview","path":"file.txt","line":1,"character":2,"new_name":"renamed"})).await);
    let result: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 5);
    assert_eq!(result["omitted"], 1);
    assert_eq!(result["results"][0]["document_version"], 1);
    assert!(
        result["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["new_text"] == "renamed")
    );
    assert_eq!(fixture.read("file.txt"), "hello\nworld\nthird\n");
    assert_eq!(fixture.read("target.txt"), "target\nsecond\n");
    assert!(!fixture.repo.join("created.txt").exists());
    assert!(!fixture.repo.join("moved.txt").exists());
    assert_eq!(fixture.read("folder/nested.txt"), "preserved folder");
    assert!(!fixture.repo.join("renamed-folder").exists());
    assert!(
        result["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["server"] == "fixture" && row["server_root"] == ".")
    );
    assert!(!fixture.repo.join("save-events").exists());
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

#[tokio::test]
async fn navigation_diagnostics_use_the_revalidated_typed_cache() {
    let (fixture, locations) = navigation_fixture("navigation");
    ok(fixture
        .call(
            "default",
            "lsp",
            json!({"operation":"definition","path":"file.txt","line":1,"character":2}),
        )
        .await);
    for path in [Some("file.txt"), None] {
        let mut input = json!({"operation":"diagnostics"});
        if let Some(path) = path {
            input["path"] = json!(path);
        }
        let output = ok(fixture.call("default", "lsp", input).await);
        let result: Value = serde_json::from_str(&output).unwrap();
        let rows = result["results"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "{result}");
        assert_eq!(rows[0]["line"], 3);
        assert_eq!(rows[0]["character"], 4);
        assert_eq!(rows[0]["preview"], "third");
        assert!(
            rows.iter()
                .all(|row| row["server_version"] == 1 && row["observed_document_version"] == 1)
        );
    }
    fixture.write("file.txt", "user changed\n");
    let output = ok(fixture
        .call("default", "lsp", json!({"operation":"diagnostics"}))
        .await);
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["results"],
        json!([])
    );
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

#[tokio::test]
async fn cold_workspace_symbols_need_no_document_and_denied_requests_start_nothing() {
    let (fixture, locations) = navigation_fixture("navigation");
    fixture.set_config(json!({"permissions":{"lsp":"deny"}}));
    assert!(
        support::failed(
            fixture
                .call(
                    "bypass",
                    "lsp",
                    json!({"operation":"workspace_symbols","query":"find"})
                )
                .await
        )
        .contains("denied")
    );
    assert!(locations.status(&fixture.repo).unwrap().is_empty());
    fixture.set_config(json!({"permissions":{"lsp":"allow"}}));
    let output = ok(fixture
        .call(
            "default",
            "lsp",
            json!({"operation":"workspace_symbols","query":"find"}),
        )
        .await);
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["results"][0]["name"],
        "workspace"
    );
    assert!(!fixture.repo.join("sync-events").exists());
    for mode in ["default", "plan"] {
        for patch in [false, true] {
            assert!(
                fixture
                    .tool_names(mode, patch)
                    .iter()
                    .any(|name| name == "lsp")
            );
        }
    }
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

#[tokio::test]
async fn navigation_refuses_disabled_external_invalid_and_remote_failure_inputs() {
    let (fixture, locations) = navigation_fixture("navigation-error");
    for input in [
        json!({"operation":"definition","path":"file.txt","line":0,"character":1}),
        json!({"operation":"rename_preview","path":"file.txt","line":1,"character":2}),
        json!({"operation":"definition","path":"missing.txt","line":1,"character":2}),
    ] {
        support::failed(fixture.call("default", "lsp", input).await);
    }
    let outside = fixture.dir.path().join("outside.txt");
    std::fs::write(&outside, "private").unwrap();
    assert!(
        support::failed(
            fixture
                .call(
                    "default",
                    "lsp",
                    json!({"operation":"definition","path":outside,"line":1,"character":2})
                )
                .await
        )
        .contains("inside the selected Location")
    );
    fixture.set_config(json!({"lsp":false,"permissions":{"lsp":"allow"}}));
    assert!(
        support::failed(
            fixture
                .call(
                    "default",
                    "lsp",
                    json!({"operation":"workspace_symbols","query":"find"})
                )
                .await
        )
        .contains("disabled")
    );
    assert!(locations.status(&fixture.repo).unwrap().is_empty());
    fixture.set_config(json!({"permissions":{"lsp":"allow"}}));
    let output = support::failed(
        fixture
            .call(
                "default",
                "lsp",
                json!({"operation":"definition","path":"file.txt","line":1,"character":2}),
            )
            .await,
    );
    assert!(output.contains("failed or do not support"));
    assert!(!output.contains("private-error-secret"));
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

#[tokio::test]
async fn navigation_discards_response_after_user_changes_the_source() {
    use cyber_server::runtime::ToolHost;
    use tokio_util::sync::CancellationToken;
    let (fixture, locations) = navigation_fixture("navigation-delay");
    let host = fixture.host.clone();
    let invocation = fixture.invocation(
        "default",
        "lsp",
        json!({"operation":"definition","path":"file.txt","line":1,"character":2}),
    );
    let pending =
        tokio::spawn(async move { host.execute(invocation, CancellationToken::new()).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !fixture.repo.join("rpc-events").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    fixture.write("file.txt", "user changed during request\n");
    assert!(support::failed(pending.await.unwrap()).contains("source changed"));
    assert_eq!(fixture.read("file.txt"), "user changed during request\n");
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

#[tokio::test]
async fn navigation_preserves_literal_backslashes_and_unicode_in_relative_paths() {
    let (fixture, locations) = navigation_fixture("navigation");
    let name = "literal\\λ.txt";
    fixture.write(name, "hello\nworld\nthird\n");
    let output = ok(fixture
        .call(
            "default",
            "lsp",
            json!({"operation":"implementation","path":name,"line":1,"character":2}),
        )
        .await);
    let result: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(result["results"][0]["path"], name);
    assert_eq!(result["results"][0]["preview"], "world");
    assert!(
        locations
            .close()
            .await
            .unwrap()
            .iter()
            .all(|s| s.acknowledged)
    );
}

#[tokio::test]
async fn unapproved_navigation_obeys_default_ask_and_dont_ask_without_startup() {
    let (fixture, locations) = navigation_fixture("navigation");
    fixture.set_config(json!({}));
    for mode in ["default", "plan", "dont-ask"] {
        support::failed(
            fixture
                .call(
                    mode,
                    "lsp",
                    json!({"operation":"workspace_symbols","query":"find"}),
                )
                .await,
        );
    }
    assert!(locations.status(&fixture.repo).unwrap().is_empty());
    assert!(!fixture.repo.join("rpc-events").exists());
    assert!(locations.close().await.unwrap().is_empty());
}
