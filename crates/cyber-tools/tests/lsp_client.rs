use cyber_tools::lsp::{Framed, LspError, StdioClient};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{DuplexStream, ReadHalf, WriteHalf, duplex, split};

type Server = Framed<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;
type Client = StdioClient<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;

fn pair(root: &std::path::Path) -> (Client, Server) {
    let (client, server) = duplex(4096);
    let (reader, writer) = split(client);
    let (server_reader, server_writer) = split(server);
    (
        StdioClient::new(reader, writer, root).unwrap(),
        Framed::new(server_reader, server_writer),
    )
}

async fn initialize(server: &mut Server) {
    let request = server.read().await.unwrap().unwrap();
    assert_eq!(request["method"], "initialize");
    assert_eq!(
        request["params"]["capabilities"],
        json!({"workspace":{"workspaceFolders":true},"textDocument":{"publishDiagnostics":{"versionSupport":true}}})
    );
    assert!(
        request["params"]["rootUri"]
            .as_str()
            .unwrap()
            .starts_with("file:")
    );
    server.write(&json!({"jsonrpc":"2.0","id":request["id"],"result":{"capabilities":{"hoverProvider":true}}})).await.unwrap();
    assert_eq!(
        server.read().await.unwrap().unwrap()["method"],
        "initialized"
    );
}

#[tokio::test]
async fn lifecycle_admission_and_workspace_folder_request_use_the_immutable_root() {
    let root = tempfile::tempdir().unwrap();
    let (mut client, mut server) = pair(root.path());
    assert!(
        client
            .request("fixture", Value::Null, Duration::from_secs(1))
            .await
            .is_err()
    );
    let fixture = tokio::spawn(async move {
        initialize(&mut server).await;
        let request = server.read().await.unwrap().unwrap();
        server
            .write(&json!({"jsonrpc":"2.0","id":"root","method":"workspace/workspaceFolders"}))
            .await
            .unwrap();
        let root_reply = server.read().await.unwrap().unwrap();
        assert!(
            root_reply["result"][0]["uri"]
                .as_str()
                .unwrap()
                .starts_with("file:")
        );
        server
            .write(&json!({"jsonrpc":"2.0","id":request["id"],"result":null}))
            .await
            .unwrap();
    });
    client
        .initialize(Value::Null, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(
        client
            .initialize(Value::Null, Duration::from_secs(1))
            .await
            .is_err()
    );
    assert!(
        client
            .request("shutdown", Value::Null, Duration::from_secs(1))
            .await
            .is_err()
    );
    assert!(
        client
            .request("fixture", Value::Null, Duration::from_secs(1))
            .await
            .unwrap()
            .is_null()
    );
    fixture.await.unwrap();
}

#[tokio::test]
async fn invalid_response_id_or_envelope_permanently_fences_request_ownership() {
    for response in [
        json!({"id":99,"result":null}),
        json!({"id":2,"result":null,"error":{"code":-1,"message":"x"}}),
        json!({"id":2,"error":{"code":-1}}),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (mut client, mut server) = pair(root.path());
        let fixture = tokio::spawn(async move {
            initialize(&mut server).await;
            server.read().await.unwrap().unwrap();
            let mut response = response;
            response["jsonrpc"] = json!("2.0");
            server.write(&response).await.unwrap();
        });
        client
            .initialize(Value::Null, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(
            client
                .request("fixture", Value::Null, Duration::from_secs(1))
                .await
                .is_err()
        );
        assert!(
            client
                .request("fixture", Value::Null, Duration::from_secs(1))
                .await
                .is_err()
        );
        assert!(
            client
                .notify("textDocument/didSave", Value::Null)
                .await
                .is_err()
        );
        fixture.await.unwrap();
    }
}

#[tokio::test]
async fn outer_future_cancellation_cannot_admit_a_second_request() {
    let root = tempfile::tempdir().unwrap();
    let (mut client, mut server) = pair(root.path());
    let fixture = tokio::spawn(async move {
        initialize(&mut server).await;
        assert_eq!(server.read().await.unwrap().unwrap()["method"], "hang");
        assert!(server.read().await.unwrap().is_none());
    });
    client
        .initialize(Value::Null, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            client.request("hang", Value::Null, Duration::from_secs(1))
        )
        .await
        .is_err()
    );
    assert!(
        client
            .request("fixture", Value::Null, Duration::from_secs(1))
            .await
            .is_err()
    );
    drop(client);
    fixture.await.unwrap();
}

#[tokio::test]
async fn server_notification_flood_is_bounded_and_fences_reuse() {
    let root = tempfile::tempdir().unwrap();
    let (mut client, mut server) = pair(root.path());
    let fixture = tokio::spawn(async move {
        initialize(&mut server).await;
        server.read().await.unwrap().unwrap();
        for _ in 0..129 {
            server.write(&json!({"jsonrpc":"2.0","method":"window/logMessage","params":{"message":"fixture"}})).await.unwrap();
        }
    });
    client
        .initialize(Value::Null, Duration::from_secs(1))
        .await
        .unwrap();
    let error = client
        .request("fixture", Value::Null, Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        LspError::Protocol("notification limit exceeded")
    ));
    assert_eq!(client.take_notifications().len(), 128);
    assert!(
        client
            .request("fixture", Value::Null, Duration::from_secs(1))
            .await
            .is_err()
    );
    fixture.await.unwrap();
}
