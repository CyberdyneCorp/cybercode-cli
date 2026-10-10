//! Real framed server feedback through the public tool host.
#![cfg(unix)]

mod support;

use cyber_core::paths::Paths;
use cyber_tools::lsp::{LaunchOptions, LocalLauncher, Locations};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use support::{Fixture, ok};

const SERVER: &str = r#"
import json,sys,pathlib,urllib.parse
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
    elif method in ['textDocument/didOpen','textDocument/didChange']:
        d=p['textDocument']; versions[d['uri']]=d['version']
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
    std::fs::write(paths.config.join("cyber.jsonc"), json!({"lsp":{"fixture":{"command":["python3","-u","-c",SERVER,mode],"extensions":[".txt",".ipynb"]}},"sandbox":{"network":"off"}}).to_string()).unwrap();
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
