//! Large paste and image input (`tui` → Large paste and image input), through a
//! production App over HTTP with a local scripted model: paste chips, dropped paths, `@`
//! media mentions, the clipboard binding, drafts and refused prompts.

use std::time::Duration;

use base64::Engine as _;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::app::{Action, App};
use crate::attachments::{ClipboardError, Fake};
use crate::screens::{Opts, Server, render};

/// A 1×1 PNG.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
#[cfg(unix)]
const PDF: &[u8] = b"%PDF-1.4\n1 0 obj << >> endobj\ntrailer << >>\n%%EOF\n";

fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(PNG)
        .unwrap()
}

/// `main` takes images and PDFs; `text` neither.
async fn server() -> Server {
    Server::start_models(
        json!({
            "main": {"capabilities": {"vision": true, "pdf": true}},
            "text": {},
        }),
        json!({}),
        false,
    )
    .await
}

async fn session_on(server: &Server, model: &str) -> String {
    let created = server
        .client
        .post("/sessions", json!({ "model": model, "mode": "default" }))
        .await
        .unwrap();
    created["data"]["id"].as_str().unwrap().to_owned()
}

/// A production App on `id`, with its clipboard images kept in `state`.
async fn app(server: &Server, id: &str, state: &std::path::Path) -> App {
    let mut app = server.load(id).await;
    app.attachments.dir = Some(state.to_path_buf());
    app
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, modifiers))
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

fn enter(app: &mut App) -> Vec<Action> {
    app.completion = None;
    press(app, KeyCode::Enter, KeyModifiers::NONE)
}

/// Perform `actions` as the runner does, until nothing follows.
async fn drive(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    while !actions.is_empty() {
        let action = actions.remove(0);
        if matches!(action, Action::FindFiles(_) | Action::LoadAgents) {
            continue;
        }
        let msg = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
}

/// The first model request whose body contains `needle`.
async fn request_with(server: &Server, needle: &str) -> Value {
    for _ in 0..1000 {
        if let Some(body) = server
            .model
            .requests()
            .into_iter()
            .find(|body| body.to_string().contains(needle))
        {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no model request contains {needle:?}");
}

async fn wait_idle(server: &Server, id: &str) {
    for _ in 0..1000 {
        if !server.host.runtime.is_running(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} kept running");
}

fn screen(app: &mut App) -> String {
    let directory = app.session.directory.clone();
    let screen = render(app, 100, 20, &Opts::default());
    app.session.directory = directory;
    screen
}

fn log(lines: usize, tag: &str) -> String {
    (1..=lines)
        .map(|n| format!("{tag} log line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn large_paste_becomes_a_chip_and_submits_in_full() {
    let server = server().await;
    let id = server.session().await;
    let state = tempfile::tempdir().unwrap();
    let mut app = app(&server, &id, state.path()).await;

    // 300 lines become one chip, and the prompt carries all of them.
    app.on_paste(&log(300, "first"));
    assert_eq!(app.composer.text(), "[Pasted text #1 +300 lines]");
    assert!(screen(&mut app).contains("[Pasted text #1 +300 lines]"));
    let actions = enter(&mut app);
    drive(&server, &mut app, actions).await;
    let sent = request_with(&server, "first log line 300")
        .await
        .to_string();
    assert!(sent.contains("first log line 1\\n"), "{sent}");
    assert!(
        !sent.contains("[Pasted text #1"),
        "the label is expanded: {sent}"
    );
    wait_idle(&server, &id).await;

    // With `paste_summary: false` the paste is typed in.
    app.attachments.paste_summary = false;
    let second = log(300, "second");
    app.on_paste(&second);
    assert_eq!(app.composer.text(), second);
    assert!(app.attachments.chips.is_empty());
    press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    app.attachments.paste_summary = true;

    // Backspace right after a chip removes all of it.
    app.on_paste(&log(30, "third"));
    assert_eq!(app.composer.text(), "[Pasted text #1 +30 lines]");
    press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(app.composer.text(), "");
    assert!(app.attachments.chips.is_empty());

    // A saved draft comes back with its chip on the next start.
    typed(&mut app, "see ");
    app.on_paste(&log(40, "fourth"));
    let store = crate::store::LocalStore::new(state.path());
    crate::runner::save_draft(&store, &app);
    let opts = crate::TuiOptions {
        client: server.client.clone(),
        directory: server.root().display().to_string(),
        state_dir: state.path().to_path_buf(),
        config_dir: state.path().join("config"),
        start: crate::Start::Resume(id.clone()),
        fork: false,
        model: None,
        agent: None,
        mode: None,
        prompt: None,
    };
    let (mut restored, _) = crate::runner::start(&server.client, &store, &opts, &|_| None)
        .await
        .unwrap();
    // Numbers are not reused within a draft: #1 was deleted above.
    assert_eq!(restored.composer.text(), "see [Pasted text #2 +40 lines]");
    let actions = enter(&mut restored);
    drive(&server, &mut restored, actions).await;
    let sent = request_with(&server, "fourth log line 40")
        .await
        .to_string();
    assert!(sent.contains("see fourth log line 1\\n"), "{sent}");
}

/// `tui` → Composer appearance: pending chips render in `accent` on `selection` (reverse
/// video under `NO_COLOR`), and the text typed around them keeps the composer's style.
#[tokio::test]
async fn chips_render_in_accent_on_selection() {
    use crate::screens::buffer;
    use crate::screens_mouse::cell_of;
    use ratatui::style::Modifier;
    let server = server().await;
    let id = server.session().await;
    let state = tempfile::tempdir().unwrap();
    let mut app = app(&server, &id, state.path()).await;
    typed(&mut app, "see ");
    app.on_paste(&log(30, "chip"));
    typed(&mut app, " and [Pasted text #9 +1 lines]");

    let buf = buffer(&mut app, 120, 30, &Opts::default());
    let (x, y) = cell_of(&buf, "[Pasted text #1 +30 lines]");
    let label = "[Pasted text #1 +30 lines]".len() as u16;
    let (accent, selection) = (app.theme.accent, app.theme.selection);
    for dx in [0, label - 1] {
        let cell = &buf[(x + dx, y)];
        assert_eq!((cell.fg, cell.bg), (accent, selection), "{dx}");
    }
    // Typed text, including a label no chip stands for, is not a chip.
    let typed_text = [x - 2, x + label + 1, cell_of(&buf, "[Pasted text #9").0];
    for column in typed_text {
        assert_eq!(buf[(column, y)].fg, app.theme.text, "{column}");
        assert_eq!(buf[(column, y)].bg, app.theme.surface, "{column}");
    }

    let no_color = Opts {
        no_color: true,
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 30, &no_color);
    let (x, y) = cell_of(&buf, "[Pasted text #1 +30 lines]");
    assert!(buf[(x, y)].modifier.contains(Modifier::REVERSED));
    assert!(!buf[(x - 2, y)].modifier.contains(Modifier::REVERSED));
}

#[cfg(unix)]
#[tokio::test]
async fn dropped_paths_and_mentions_become_chips() {
    let server = server().await;
    let id = server.session().await;
    let state = tempfile::tempdir().unwrap();
    let root = server.root();
    let shot = root.join("Screen Shot.png");
    std::fs::write(&shot, png()).unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/spec.pdf"), PDF).unwrap();
    let huge = root.join("huge.png");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(25 * 1024 * 1024)
        .unwrap();
    let mut app = app(&server, &id, state.path()).await;

    // A drop pastes a quoted path; a backslash-escaped or file:// one works too.
    app.on_paste(&format!("'{}'", shot.display()));
    assert_eq!(app.composer.text(), "[Image #1] ");
    app.on_paste(&shot.display().to_string().replace(' ', "\\ "));
    app.on_paste(&format!(
        "file://{}",
        shot.display().to_string().replace(' ', "%20")
    ));
    assert_eq!(app.composer.text(), "[Image #1] [Image #2] [Image #3] ");
    assert!(screen(&mut app).contains("[Image #1] [Image #2] [Image #3]"));
    let chips = app.attachments.take(&app.composer.text());
    let (_, parts) =
        crate::attachments::submit_parts("", &chips, &app.session.directory, true).unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0]["type"], "file");
    assert_eq!(parts[0]["path"], shot.display().to_string());
    assert_eq!(parts[0]["name"], "Screen Shot.png");
    press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);

    // Too large: a toast, and nothing attached.
    app.on_paste(&format!("'{}'", huge.display()));
    assert_eq!(app.composer.text(), "");
    assert!(app.attachments.chips.is_empty());
    assert_eq!(app.toast.as_ref().unwrap().0, "Attachment exceeds 20 MiB");

    // A drop sent to the server: it reads the file and the model gets the image.
    app.on_paste(&format!("'{}'", shot.display()));
    typed(&mut app, "describe");
    let actions = enter(&mut app);
    drive(&server, &mut app, actions).await;
    request_with(&server, PNG.trim_end_matches('=')).await;
    wait_idle(&server, &id).await;

    // Picking a PDF from the `@` menu inserts a chip in place of the mention.
    app.files = vec!["docs/spec.pdf".into()];
    typed(&mut app, "summarize @docs/sp");
    assert!(app.completion.is_some());
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(app.composer.text(), "summarize [PDF #1] ");
    let actions = enter(&mut app);
    drive(&server, &mut app, actions).await;
    // `JVBERi0x` is the base64 of `%PDF-1`.
    request_with(&server, "JVBERi0x").await;
    wait_idle(&server, &id).await;

    // The transcript names both attachments.
    let msg = crate::perform::snapshot(&server.client, &id).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    let shown = format!("{:?}", app.items);
    assert!(shown.contains("[Image: Screen Shot.png"), "{shown}");
    assert!(shown.contains("[PDF: spec.pdf"), "{shown}");
}

/// The default `composer.paste_image` key.
fn paste_key() -> (KeyCode, KeyModifiers) {
    if cfg!(windows) {
        (KeyCode::Char('v'), KeyModifiers::ALT)
    } else {
        (KeyCode::Char('v'), KeyModifiers::CONTROL)
    }
}

#[tokio::test]
async fn ctrl_v_attaches_the_clipboard_image() {
    let server = server().await;
    let id = server.session().await;
    let state = tempfile::tempdir().unwrap();
    let mut app = app(&server, &id, state.path()).await;
    let (code, modifiers) = paste_key();

    // An image: a chip backed by a private file, sent inline.
    app.attachments.clipboard = Box::new(Fake(Ok(Some(png()))));
    press(&mut app, code, modifiers);
    assert_eq!(app.composer.text(), "[Image #1] ");
    let crate::attachments::Payload::Clipboard { file } = app.attachments.chips[0].payload.clone()
    else {
        panic!("a clipboard chip");
    };
    assert!(std::path::Path::new(&file).starts_with(state.path()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    typed(&mut app, "what is this");
    let actions = enter(&mut app);
    drive(&server, &mut app, actions).await;
    request_with(&server, PNG.trim_end_matches('=')).await;
    assert!(
        !std::path::Path::new(&file).exists(),
        "the private file goes once sent"
    );
    wait_idle(&server, &id).await;

    // No image, or no tool: a toast and an unchanged composer.
    app.attachments.clipboard = Box::new(Fake(Ok(None)));
    press(&mut app, code, modifiers);
    assert_eq!(app.composer.text(), "");
    assert_eq!(app.toast.as_ref().unwrap().0, "No image on the clipboard");
    app.attachments.clipboard = Box::new(Fake(Err(ClipboardError::MissingTool(
        "Clipboard image needs wl-paste or xclip",
    ))));
    press(&mut app, code, modifiers);
    assert_eq!(app.composer.text(), "");
    assert_eq!(
        app.toast.as_ref().unwrap().0,
        "Clipboard image needs wl-paste or xclip"
    );

    // Rebound: the new key attaches, and the default no longer reads the clipboard.
    let mut warnings = Vec::new();
    let keybinds = json!({ "composer": { "paste_image": "ctrl+shift+i" } });
    app.keymap = crate::keys::Keymap::load(Some(&keybinds), &mut warnings);
    assert!(warnings.is_empty(), "{warnings:?}");
    app.attachments.clipboard = Box::new(Fake(Ok(Some(png()))));
    press(&mut app, code, modifiers);
    assert!(app.attachments.chips.is_empty());
    press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    press(
        &mut app,
        KeyCode::Char('I'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(app.composer.text(), "[Image #1] ");
}

#[tokio::test]
async fn refused_prompt_keeps_chips() {
    let server = server().await;
    let id = session_on(&server, "test/text").await;
    let state = tempfile::tempdir().unwrap();
    let mut app = app(&server, &id, state.path()).await;
    app.attachments.clipboard = Box::new(Fake(Ok(Some(png()))));
    let (code, modifiers) = paste_key();
    press(&mut app, code, modifiers);
    typed(&mut app, "explain");
    let actions = enter(&mut app);
    assert_eq!(app.composer.text(), "");
    drive(&server, &mut app, actions).await;
    assert_eq!(app.composer.text(), "[Image #1] explain");
    assert_eq!(app.attachments.chips.len(), 1);
    let toast = app.toast.as_ref().unwrap().0.clone();
    assert!(toast.contains("has no image input"), "{toast}");
    assert!(
        !toast.contains("Error:"),
        "the server's words, no tag: {toast}"
    );
    let crate::attachments::Payload::Clipboard { file } = &app.attachments.chips[0].payload else {
        panic!("a clipboard chip");
    };
    assert!(std::path::Path::new(file).is_file(), "the image stays");
    assert!(
        server
            .model
            .requests()
            .iter()
            .all(|body| !body.to_string().contains("explain")),
        "nothing reached the model"
    );
}

#[tokio::test]
async fn taking_back_a_queued_image_replaces_the_draft_chips() {
    let server = server().await;
    let id = server.session().await;
    server.prompt(&id, "HOLD on").await;
    for _ in 0..1000 {
        if server.host.runtime.is_running(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let queued = json!({
        "delivery": "queue",
        "parts": [
            {"type": "text", "text": "[Image #1] queued"},
            {"type": "image", "media_type": "image/png", "data": PNG},
        ],
    });
    server
        .client
        .post(&format!("/sessions/{id}/prompt"), queued)
        .await
        .unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut app = app(&server, &id, state.path()).await;
    assert_eq!(app.queued.len(), 1, "{:?}", app.queued);

    // The draft holds a clipboard `[Image #1]`; Alt+Up replaces it with the queued input.
    app.attachments.clipboard = Box::new(Fake(Ok(Some(png()))));
    let (code, modifiers) = paste_key();
    press(&mut app, code, modifiers);
    let crate::attachments::Payload::Clipboard { file } = app.attachments.chips[0].payload.clone()
    else {
        panic!("a clipboard chip");
    };
    press(&mut app, KeyCode::Up, KeyModifiers::ALT);
    assert_eq!(app.composer.text(), "[Image #1] queued");
    assert!(
        !std::path::Path::new(&file).exists(),
        "the replaced draft's file goes"
    );
    let actions = enter(&mut app);
    let [Action::Prompt { chips, .. }] = actions.as_slice() else {
        panic!("{actions:?}");
    };
    assert_eq!(chips.len(), 1, "one label, one attachment: {chips:?}");
    assert!(matches!(
        chips[0].payload,
        crate::attachments::Payload::Stored { .. }
    ));
    server.model.release();
}

#[tokio::test]
async fn commands_and_shells_get_pasted_text_in_full() {
    let server = server().await;
    let id = server.session().await;
    let state = tempfile::tempdir().unwrap();
    let mut app = app(&server, &id, state.path()).await;
    let pasted = log(300, "diff");

    // A slash command argument carries the pasted text, not its label.
    typed(&mut app, "/deploy ");
    app.on_paste(&pasted);
    assert_eq!(app.composer.text(), "/deploy [Pasted text #1 +300 lines]");
    let actions = enter(&mut app);
    let [
        Action::Command {
            name, arguments, ..
        },
    ] = actions.as_slice()
    else {
        panic!("a command: {actions:?}");
    };
    assert_eq!(name, "deploy");
    assert_eq!(arguments, &pasted);

    // So does a shell command.
    typed(&mut app, "!cat ");
    app.on_paste(&pasted);
    let actions = enter(&mut app);
    let [Action::Shell(command)] = actions.as_slice() else {
        panic!("a shell command: {actions:?}");
    };
    assert_eq!(command, &format!("cat {pasted}"));

    // Media cannot go to a command: the draft and its chip stay.
    app.attachments.clipboard = Box::new(Fake(Ok(Some(png()))));
    typed(&mut app, "!open ");
    let (code, modifiers) = paste_key();
    press(&mut app, code, modifiers);
    assert!(enter(&mut app).is_empty());
    assert_eq!(app.composer.text(), "!open [Image #1] ");
    assert_eq!(app.attachments.chips.len(), 1);
    let toast = app.toast.as_ref().unwrap().0.clone();
    assert!(toast.contains("cannot be sent to a command"), "{toast}");
}
