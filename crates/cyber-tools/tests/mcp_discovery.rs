//! Discovery over actual framed IO, without substituting process ownership proofs.
use cyber_core::config::McpToolFilter;
use cyber_tools::mcp::{McpError, StdioClient, exposed_tool_name};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

type Client = StdioClient<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;
type Peer = (BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>);
const TIMEOUT: Duration = Duration::from_secs(1);

fn pair() -> (Client, Peer) {
    let (client, peer) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(client);
    let (peer_read, peer_write) = tokio::io::split(peer);
    (
        StdioClient::new(read, write),
        (BufReader::new(peer_read), peer_write),
    )
}

async fn read(peer: &mut Peer) -> Value {
    let mut line = String::new();
    peer.0.read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

async fn send(peer: &mut Peer, value: Value) {
    peer.1
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
}

async fn initialize(peer: &mut Peer, capabilities: Value) {
    let request = read(peer).await;
    assert_eq!(request["method"], "initialize");
    send(peer, json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":"2025-11-25","capabilities":capabilities,"serverInfo":{"name":"fixture","version":"1"},"instructions":"Use the reviewed tools."}})).await;
    assert_eq!(read(peer).await["method"], "notifications/initialized");
}

fn tool(name: &str) -> Value {
    json!({"name":name,"description":"fixture","inputSchema":{"type":"array","properties":{"x":{"type":"number"}}},"annotations":{"readOnlyHint":true,"destructiveHint":false},"outputSchema":{"type":"object"},"extension":{"retained":true}})
}

#[test]
fn tool_names_normalize_and_truncate_with_stable_six_character_hashes() {
    assert_eq!(
        exposed_tool_name("audit", "read.file/Δ"),
        "mcp__audit__read_file__"
    );
    let first = exposed_tool_name("audit", &"x".repeat(100));
    assert_eq!(first.len(), 64);
    assert_eq!(
        &first[..57],
        &format!("mcp__audit__{}", "x".repeat(100))[..57]
    );
    assert_eq!(first.as_bytes()[57], b'_');
    assert_eq!(&first[58..], "91612e");
    assert!(first[58..].bytes().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(first, exposed_tool_name("audit", &"x".repeat(100)));
    assert_ne!(
        first,
        exposed_tool_name("audit", &format!("{}y", "x".repeat(99)))
    );
}

#[tokio::test]
async fn pagination_keeps_remote_identity_annotations_and_normalized_schema() {
    let (mut client, mut peer) = pair();
    let long_name = "read.".repeat(20);
    let remote_name = long_name.clone();
    let server = tokio::spawn(async move {
        initialize(&mut peer, json!({"tools":{},"resources":{}})).await;
        let first = read(&mut peer).await;
        assert_eq!(first["method"], "tools/list");
        assert!(first["params"].get("cursor").is_none());
        send(&mut peer, json!({"jsonrpc":"2.0","id":first["id"],"result":{"tools":[tool("read.first"),tool("delete_all")],"nextCursor":"opaque/Δ"}})).await;
        let next = read(&mut peer).await;
        assert_ne!(next["id"], first["id"]);
        assert_eq!(next["params"]["cursor"], "opaque/Δ");
        send(
            &mut peer,
            json!({"jsonrpc":"2.0","id":next["id"],"result":{"tools":[tool(&remote_name)]}}),
        )
        .await;
    });
    client.initialize(TIMEOUT).await.unwrap();
    assert_eq!(
        client.metadata().unwrap()["instructions"],
        "Use the reviewed tools."
    );
    assert!(client.supports("resources"));
    let filter = McpToolFilter {
        allow: Some(vec!["read*".into(), "delete*".into()]),
        deny: vec!["delete*".into()],
    };
    let tools = client
        .discover_tools("audit", &filter, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].remote_name, "read.first");
    assert_eq!(tools[0].exposed_name, "mcp__audit__read_first");
    assert_eq!(tools[1].remote_name, long_name);
    assert_eq!(tools[1].exposed_name.len(), 64);
    assert_eq!(tools[0].spec().input_schema["type"], "object");
    assert_eq!(
        tools[0].spec().input_schema["properties"]["x"]["type"],
        "number"
    );
    assert!(tools[0].read_only_hint());
    assert_eq!(tools[0].destructive_hint(), Some(false));
    assert_eq!(tools[0].definition["outputSchema"]["type"], "object");
    assert_eq!(tools[0].definition["extension"]["retained"], true);
    assert!(!client.unresolved());
    server.await.unwrap();
}

#[tokio::test]
async fn normalized_collisions_keep_all_tools_and_are_stable_across_listing_order() {
    let mut previous = None;
    for names in [
        vec!["read.file", "read_file", "read_file_fbb1e4"],
        vec!["read_file_fbb1e4", "read_file", "read.file"],
    ] {
        let (mut client, mut peer) = pair();
        let server = tokio::spawn(async move {
            initialize(&mut peer, json!({"tools":{}})).await;
            let request = read(&mut peer).await;
            let tools: Vec<Value> = names.into_iter().map(tool).collect();
            send(
                &mut peer,
                json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":tools}}),
            )
            .await;
        });
        client.initialize(TIMEOUT).await.unwrap();
        let tools = client
            .discover_tools("audit", &McpToolFilter::default(), TIMEOUT)
            .await
            .unwrap();
        let mapping: std::collections::BTreeMap<_, _> = tools
            .into_iter()
            .map(|tool| (tool.remote_name, tool.exposed_name))
            .collect();
        assert_eq!(mapping.len(), 3);
        assert_eq!(
            mapping
                .values()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        assert_eq!(mapping["read_file_fbb1e4"], "mcp__audit__read_file_fbb1e4");
        assert_ne!(
            mapping["read.file"], "mcp__audit__read_file_fbb1e4",
            "hashed collision cannot shadow an existing tool"
        );
        assert!(mapping.values().all(|name| name.len() <= 64));
        if let Some(previous) = previous {
            assert_eq!(mapping, previous);
        }
        previous = Some(mapping);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn absent_tools_capability_sends_no_listing_request() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        initialize(&mut peer, json!({"resources":{}})).await;
        let mut line = String::new();
        assert_eq!(peer.0.read_line(&mut line).await.unwrap(), 0);
    });
    client.initialize(TIMEOUT).await.unwrap();
    assert!(
        client
            .discover_tools("audit", &McpToolFilter::default(), TIMEOUT)
            .await
            .unwrap()
            .is_empty()
    );
    drop(client);
    server.await.unwrap();
}

#[tokio::test]
async fn malformed_or_duplicate_catalogs_are_refused_without_partial_results() {
    let pages = [
        json!({"tools":null}),
        json!({"tools":[tool("read"),tool("read")]}),
        json!({"tools":[{"name":"bad","inputSchema":null}]}),
        json!({"tools":[{"name":"bad","inputSchema":{},"annotations":{"readOnlyHint":"true"}}]}),
        json!({"tools":[],"nextCursor":false}),
    ];
    for page in pages {
        let (mut client, mut peer) = pair();
        let server = tokio::spawn(async move {
            initialize(&mut peer, json!({"tools":{}})).await;
            let request = read(&mut peer).await;
            send(
                &mut peer,
                json!({"jsonrpc":"2.0","id":request["id"],"result":page}),
            )
            .await;
        });
        client.initialize(TIMEOUT).await.unwrap();
        assert!(
            client
                .discover_tools("audit", &McpToolFilter::default(), TIMEOUT)
                .await
                .is_err()
        );
        assert!(
            !client.unresolved(),
            "RPC completion is separate from catalog validity"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn pagination_cycle_is_refused_after_completed_responses() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        initialize(&mut peer, json!({"tools":{}})).await;
        for _ in 0..2 {
            let request = read(&mut peer).await;
            send(&mut peer,json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[],"nextCursor":"cycle"}})).await;
        }
    });
    client.initialize(TIMEOUT).await.unwrap();
    assert!(matches!(
        client
            .discover_tools("audit", &McpToolFilter::default(), TIMEOUT)
            .await,
        Err(McpError::Protocol("repeated MCP tool listing cursor"))
    ));
    assert!(!client.unresolved());
    server.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn pages_and_progress_share_one_absolute_listing_deadline() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        initialize(&mut peer, json!({"tools":{}})).await;
        let request = read(&mut peer).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        send(
            &mut peer,
            json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[],"nextCursor":"next"}}),
        )
        .await;
        let request = read(&mut peer).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        send(&mut peer,json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":request["id"],"progress":1}})).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        send(
            &mut peer,
            json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[]}}),
        )
        .await;
    });
    client.initialize(TIMEOUT).await.unwrap();
    let started = tokio::time::Instant::now();
    assert!(matches!(
        client
            .discover_tools("audit", &McpToolFilter::default(), TIMEOUT)
            .await,
        Err(McpError::Timeout)
    ));
    assert_eq!(started.elapsed(), TIMEOUT);
    assert!(client.unresolved());
    assert!(client.list_tools(None, TIMEOUT).await.is_err());
    server.await.unwrap();
}

#[tokio::test]
async fn invalid_advertised_capabilities_or_instructions_do_not_initialize() {
    for field in [
        json!({"prompts":true}),
        json!({"resources":false}),
        json!({"tools":null}),
    ] {
        let (mut client, mut peer) = pair();
        let server = tokio::spawn(async move {
            let request = read(&mut peer).await;
            send(&mut peer,json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":"2025-11-25","capabilities":field,"serverInfo":{"name":"fixture","version":"1"}}})).await;
        });
        assert!(client.initialize(TIMEOUT).await.is_err());
        assert!(client.metadata().is_none());
        assert!(client.unresolved());
        server.await.unwrap();
    }
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        let request = read(&mut peer).await;
        send(&mut peer,json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"fixture","version":"1"},"instructions":{"not":"text"}}})).await;
    });
    assert!(client.initialize(TIMEOUT).await.is_err());
    assert!(client.metadata().is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn aggregate_catalog_is_bounded_even_when_every_frame_fits() {
    let (mut client, mut peer) = pair();
    let server = tokio::spawn(async move {
        initialize(&mut peer, json!({"tools":{}})).await;
        let mut count = 0;
        loop {
            let mut line = String::new();
            if peer.0.read_line(&mut line).await.unwrap() == 0 {
                return count;
            }
            let request: Value = serde_json::from_str(&line).unwrap();
            count += 1;
            let mut definition = tool(&format!("read_{count}"));
            definition["description"] = json!("x".repeat(700 * 1024));
            send(&mut peer,json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[definition],"nextCursor":count.to_string()}})).await;
        }
    });
    client.initialize(TIMEOUT).await.unwrap();
    assert!(matches!(
        client
            .discover_tools("audit", &McpToolFilter::default(), Duration::from_secs(20))
            .await,
        Err(McpError::Protocol("MCP tool catalog exceeds byte limit"))
    ));
    assert!(!client.unresolved());
    drop(client);
    assert_eq!(server.await.unwrap(), 24);
}

#[tokio::test]
async fn disposed_listing_retains_pending_identity_and_refuses_reuse() {
    let (mut client, mut peer) = pair();
    let initialized = tokio::spawn(async move {
        initialize(&mut peer, json!({"tools":{}})).await;
        peer
    });
    assert!(
        client
            .discover_tools("audit", &McpToolFilter::default(), TIMEOUT)
            .await
            .is_err()
    );
    client.initialize(TIMEOUT).await.unwrap();
    let mut peer = initialized.await.unwrap();
    let filter = McpToolFilter::default();
    let mut listing = Box::pin(client.discover_tools("audit", &filter, TIMEOUT));
    assert!(futures::poll!(&mut listing).is_pending());
    assert_eq!(read(&mut peer).await["method"], "tools/list");
    drop(listing);
    assert!(client.unresolved());
    assert!(client.list_tools(None, TIMEOUT).await.is_err());
}
