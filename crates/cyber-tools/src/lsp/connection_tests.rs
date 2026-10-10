use super::*;
use serde_json::json;
use std::process::Stdio;

const SERVER: &str = r#"
import json, sys, pathlib, subprocess, time, urllib.parse
mode = sys.argv[1]
def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        if line == b'\r\n': break
        name, value = line.decode('ascii').split(':', 1)
        if name.lower() == 'content-length': length = int(value)
    return json.loads(sys.stdin.buffer.read(length))
def send(value):
    body = json.dumps(value, ensure_ascii=False).encode('utf-8')
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n' % len(body)).encode('ascii') + body)
    sys.stdout.buffer.flush()
while True:
    msg = read()
    if msg is None: break
    method = msg.get('method')
    if method == 'initialize':
        workspace = pathlib.Path(urllib.parse.unquote(urllib.parse.urlparse(msg['params']['rootUri']).path))
        if mode == 'stderr-flood':
            sys.stderr.write('E' * (1024 * 1024 + 100)); sys.stderr.flush()
        assert msg['params']['rootUri'].startswith('file:')
        assert msg['params']['initializationOptions'] == {'fixture': True}
        if mode == 'startup-hang':
            time.sleep(60)
        if mode == 'reject':
            sys.stderr.write('private-startup-secret'); sys.stderr.flush()
            send({'jsonrpc':'2.0','id':msg['id'],'error':{'code':-32002,'message':'private-startup-secret'}})
        else:
            send({'jsonrpc':'2.0','id':msg['id'],'result':{'capabilities':{'hoverProvider':True}}})
    elif method == 'initialized':
        if mode == 'exit-on-initialized': break
        if mode == 'idle-messages':
            uri = (workspace / 'file.rs').as_uri()
            send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':uri,'version':7,'diagnostics':[]}})
            send({'jsonrpc':'2.0','id':'idle-request','method':'workspace/applyEdit','params':{'edit':{'changes':{uri:[{'range':{'start':{'line':0,'character':0},'end':{'line':0,'character':0}},'newText':'unrequested edit'}]}}}})
            reply = read()
            assert reply['error']['code'] == -32601
            (workspace / 'idle-replied').write_text('refused')
        if mode == 'idle-foreign-response':
            send({'jsonrpc':'2.0','id':999,'result':True})
        if mode in ['partial-idle-header', 'partial-idle-body']:
            body = json.dumps({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':(workspace / 'file.rs').as_uri(),'version':8,'diagnostics':[]}}).encode('utf-8')
            header = ('Content-Length: %d\r\n\r\n' % len(body)).encode('ascii')
            frame = header + body
            split_at = 10 if mode == 'partial-idle-header' else len(header) + 4
            sys.stdout.buffer.write(frame[:split_at]); sys.stdout.buffer.flush()
            (workspace / 'partial-started').write_text('started')
            request = read()
            assert request['method'] == 'fixture'
            sys.stdout.buffer.write(frame[split_at:]); sys.stdout.buffer.flush()
            send({'jsonrpc':'2.0','id':request['id'],'result':{'text':'λ🦀'}})
    elif method == 'publish-diagnostics':
        send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':msg['params']})
        send({'jsonrpc':'2.0','id':msg['id'],'result':True})
    elif method == 'fixture':
        send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':'file:///untrusted','diagnostics':[]}})
        send({'jsonrpc':'2.0','id':'server-request','method':'workspace/applyEdit','params':{'edit':{}}})
        reply = read()
        assert reply['error']['code'] == -32601
        send({'jsonrpc':'2.0','id':msg['id'],'result':{'text':'λ🦀'}})
    elif method in ['textDocument/didOpen','textDocument/didChange','textDocument/didClose']:
        with (workspace / 'document-events').open('a') as events:
            events.write(json.dumps({'method':method,'params':msg['params']})+'\n')
    elif method == 'remote-error':
        send({'jsonrpc':'2.0','id':msg['id'],'error':{'code':-32602,'message':'private-error-secret'}})
    elif method == 'descendant':
        marker = msg['params']['marker']
        subprocess.Popen(['python3','-c','import time,pathlib,sys; time.sleep(0.5); pathlib.Path(sys.argv[1]).write_text("escaped")',marker])
        send({'jsonrpc':'2.0','id':msg['id'],'result':None})
    elif method == 'hang':
        time.sleep(60)
    elif method == 'delay':
        with pathlib.Path(msg['params']['marker']).open('a') as witness:
            witness.write('started\n')
        time.sleep(0.15)
        send({'jsonrpc':'2.0','id':msg['id'],'result':True})
    elif method == 'shutdown':
        assert 'params' not in msg
        if mode == 'shutdown-hang': time.sleep(60)
        send({'jsonrpc':'2.0','id':msg['id'],'result':None})
    elif method == 'exit':
        break
"#;

pub(crate) async fn process(mode: &str) -> HookCommandProcess {
    HookCommandProcess::spawn_with_stdin(
        "python3",
        &["-u".into(), "-c".into(), SERVER.into(), mode.into()],
        None,
        |command| {
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
        },
    )
    .await
    .unwrap()
}

async fn connect(mode: &str, root: &Path) -> StdioConnection {
    StdioConnection::connect_with_timeout(
        process(mode).await,
        root,
        json!({"fixture":true}),
        Duration::from_secs(3),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn handshake_requests_notifications_and_graceful_native_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let mut owner = connect("normal", root.path()).await;
    assert_eq!(owner.capabilities().unwrap()["hoverProvider"], true);
    let result = owner
        .request("fixture", json!({}), Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(result["text"], "λ🦀");
    let notifications = owner.take_notifications();
    assert_eq!(notifications.len(), 1);
    assert_eq!(
        notifications[0]["method"],
        "textDocument/publishDiagnostics"
    );
    assert!(owner.take_notifications().is_empty());
    let stop = owner.shutdown().await;
    assert!(stop.graceful && stop.acknowledged);
    assert!(
        owner
            .request("fixture", json!({}), Duration::from_secs(1))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn remote_rejection_has_safe_diagnostics_and_does_not_break_a_connected_server() {
    let root = tempfile::tempdir().unwrap();
    let mut owner = connect("normal", root.path()).await;
    let error = owner
        .request("remote-error", json!({}), Duration::from_secs(3))
        .await
        .unwrap_err();
    assert!(matches!(error, LspError::Remote(-32602)));
    assert!(!error.to_string().contains("private-error-secret"));
    assert!(
        owner
            .request("fixture", json!({}), Duration::from_secs(3))
            .await
            .is_ok()
    );
    assert!(owner.shutdown().await.acknowledged);
}

#[tokio::test]
async fn rejected_startup_returns_native_acknowledgement_without_echoing_stderr() {
    let root = tempfile::tempdir().unwrap();
    let error = match StdioConnection::connect_with_timeout(
        process("reject").await,
        root.path(),
        json!({"fixture":true}),
        Duration::from_secs(3),
    )
    .await
    {
        Ok(_) => panic!("unexpected initialization"),
        Err(error) => error,
    };
    assert!(error.shutdown.acknowledged);
    assert!(matches!(error.error, LspError::Remote(-32002)));
    assert_eq!(error.shutdown.stderr, b"private-startup-secret");
    assert!(!error.to_string().contains("private-startup-secret"));
}

#[tokio::test]
async fn stalled_startup_is_killed_and_reaped() {
    let root = tempfile::tempdir().unwrap();
    let error = match StdioConnection::connect_with_timeout(
        process("startup-hang").await,
        root.path(),
        json!({"fixture":true}),
        Duration::from_millis(100),
    )
    .await
    {
        Ok(_) => panic!("unexpected initialization"),
        Err(error) => error,
    };
    assert!(matches!(error.error, LspError::Timeout));
    assert!(error.shutdown.acknowledged);
    assert!(!error.shutdown.graceful);
}

#[tokio::test]
async fn stalled_shutdown_is_forced_and_acknowledged() {
    let root = tempfile::tempdir().unwrap();
    let mut owner = connect("shutdown-hang", root.path()).await;
    let stop = owner.settle(Duration::from_millis(50)).await;
    assert!(!stop.graceful);
    assert!(stop.acknowledged);
}

#[tokio::test]
async fn timed_out_request_fences_reuse_until_native_settlement() {
    let root = tempfile::tempdir().unwrap();
    let mut owner = connect("normal", root.path()).await;
    assert!(matches!(
        owner
            .request("hang", json!({}), Duration::from_millis(30))
            .await,
        Err(LspError::Timeout)
    ));
    assert!(
        owner
            .request("fixture", json!({}), Duration::from_secs(1))
            .await
            .is_err()
    );
    let stop = owner.shutdown().await;
    assert!(!stop.graceful && stop.acknowledged);
}

#[tokio::test]
async fn exited_leader_does_not_release_live_descendants() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("descendant-escape");
    let mut owner = connect("normal", root.path()).await;
    owner
        .request(
            "descendant",
            json!({"marker":marker}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert!(owner.shutdown().await.acknowledged);
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn stderr_is_continuously_drained_with_bounded_retention() {
    let root = tempfile::tempdir().unwrap();
    let mut owner = connect("stderr-flood", root.path()).await;
    let stop = owner.shutdown().await;
    assert!(stop.graceful && stop.acknowledged);
    assert_eq!(stop.stderr.len(), 1024 * 1024);
    assert!(stop.stderr_truncated);
}

#[tokio::test]
async fn cancelled_request_keeps_the_native_owner_until_explicit_settlement() {
    let root = tempfile::tempdir().unwrap();
    let mut owner = connect("normal", root.path()).await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(30),
            owner.request("hang", json!({}), Duration::from_secs(3))
        )
        .await
        .is_err()
    );
    let stop = owner.shutdown().await;
    assert!(!stop.graceful && stop.acknowledged);
}

#[tokio::test]
async fn dropped_owner_terminates_descendants_without_protocol_acknowledgement() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("drop-escape");
    let mut owner = connect("normal", root.path()).await;
    owner
        .request(
            "descendant",
            json!({"marker":marker}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    drop(owner);
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn startup_future_owns_the_process_tree_before_its_first_poll() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("unpolled-escape");
    let started = root.path().join("started");
    let script = r#"
import subprocess, sys, pathlib, time
subprocess.Popen(['python3','-c','import time,pathlib,sys; time.sleep(0.5); pathlib.Path(sys.argv[1]).write_text("escaped")',sys.argv[1]])
pathlib.Path(sys.argv[2]).write_text('started')
time.sleep(60)
"#;
    let process = HookCommandProcess::spawn_with_stdin(
        "python3",
        &[
            "-u".into(),
            "-c".into(),
            script.into(),
            marker.to_string_lossy().into(),
            started.to_string_lossy().into(),
        ],
        None,
        |command| {
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
        },
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let startup = StdioConnection::connect(process, root.path(), Value::Null);
    drop(startup);
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn unacknowledged_error_retains_the_actual_owner_for_settlement_retry() {
    let root = tempfile::tempdir().unwrap();
    let owner = connect("normal", root.path()).await;
    let mut error = ConnectionError {
        error: LspError::Protocol("unverified startup settlement"),
        shutdown: Shutdown::default(),
        retained: Some(Box::new(owner)),
    };
    error.shutdown.stderr = b"retained-stderr".to_vec();
    // Dropping an unpolled retry does not dispose the retained native owner.
    drop(error.retry_shutdown());
    assert!(error.retained.is_some());
    assert!(error.retry_shutdown().await);
    assert!(error.retained.is_none());
    assert_eq!(error.shutdown.stderr, b"retained-stderr");
    assert!(error.retry_shutdown().await);
}
