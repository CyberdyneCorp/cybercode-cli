//! The sidebar's MCP section (`tui` → Session sidebar): every configured server with its
//! status, transport and tool count, notes for servers that need attention, the `+N more`
//! cap and the empty state, in dark, light and `NO_COLOR`.

use serde_json::json;

use crate::app::{Action, App};
use crate::screens::{Opts, Server, render};
use crate::sidebar::{Loaded, Part, Source, parse_mcp};

async fn app_with_servers(server: &Server, servers: serde_json::Value) -> App {
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    // The production route answers first; the fixture then fixes the statuses.
    server
        .perform(&mut app, vec![Action::LoadSidebar(Part::Mcp)])
        .await;
    assert!(app.sidebar.mcp.ready().is_some(), "GET /mcp answered");
    app.sidebar
        .apply(Loaded::Mcp(Source::Ready(parse_mcp(&servers))));
    app
}

fn mixed() -> serde_json::Value {
    json!([
        {"name":"context7","status":"connected","transport":"remote","tools":["a","b"]},
        {"name":"deepwiki","status":"connecting","transport":"remote","tools":[]},
        {"name":"files","status":"failed","transport":"stdio","tools":[],
         "error":"MCP startup failed; inspect local logs\u{1b}[31m"},
        {"name":"linear","status":"needs_auth","transport":"remote","tools":[]},
        {"name":"old","status":"disabled","transport":"stdio","tools":[]},
        {"name":"corp","status":"failed","transport":"remote","tools":[],
         "error":"blocked by organization policy"},
        {"name":"github","status":"idle","transport":"remote","tools":[]},
    ])
}

#[tokio::test]
async fn every_status_renders_with_notes_and_the_overflow_line() {
    let server = Server::start().await;
    let mut app = app_with_servers(&server, mixed()).await;
    for (name, opts) in [
        (
            "mcp_dark_160x48",
            Opts {
                legend: &["●"],
                ..Opts::default()
            },
        ),
        (
            "mcp_light_160x48",
            Opts {
                light: true,
                legend: &["●"],
                ..Opts::default()
            },
        ),
        (
            "mcp_no_color_160x48",
            Opts {
                no_color: true,
                ..Opts::default()
            },
        ),
    ] {
        let screen = render(&mut app, 160, 48, &opts);
        assert!(screen.contains("▼ MCP"), "{screen}");
        assert!(
            screen.contains("● context7 remote Connected · 2 tools"),
            "{screen}"
        );
        assert!(screen.contains("● deepwiki remote Connecting"), "{screen}");
        assert!(screen.contains("● files stdio Failed"), "{screen}");
        assert!(screen.contains("MCP startup failed"), "{screen}");
        assert!(!screen.contains('\u{1b}'), "server text is sanitized");
        assert!(screen.contains("● linear remote Needs auth"), "{screen}");
        assert!(screen.contains("cyber mcp auth linear"), "{screen}");
        assert!(screen.contains("● old stdio Disabled"), "{screen}");
        assert!(screen.contains("● corp remote Blocked"), "{screen}");
        // Six rows, then the rest behind `+N more`.
        assert!(!screen.contains("● github"), "{screen}");
        assert!(screen.contains("+1 more · /status"), "{screen}");
        insta::assert_snapshot!(name, screen);
    }
}

#[tokio::test]
async fn no_configured_servers_shows_how_to_add_one() {
    let server = Server::start().await;
    let mut app = app_with_servers(&server, json!([])).await;
    let screen = render(&mut app, 160, 48, &Opts::default());
    assert!(screen.contains("▼ MCP"), "{screen}");
    assert!(screen.contains("No MCP servers"), "{screen}");
    assert!(screen.contains("add one: cyber mcp add"), "{screen}");
    insta::assert_snapshot!("mcp_empty_160x48", screen);
}
