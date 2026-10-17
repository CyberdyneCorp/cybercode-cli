//! The sidebar's Machine section (`tui` → Machine stats): `GET /system` from a production
//! App over HTTP, its bars in light, dark and `NO_COLOR`, the threshold colors and the
//! polling rules.

use std::time::Duration;

use crate::app::{Action, App};
use crate::screens::{Opts, Server, buffer, render, row_of};
use crate::sidebar::{Loaded, Machine, Part, Source};

const GB: u64 = 1024 * 1024 * 1024;

/// The mockup's machine: 82% idle on 12 cores, 71% memory and 93% disk in use.
fn sample() -> Machine {
    Machine {
        label: Some("macOS 15.0".into()),
        cores: 12,
        cpu_idle: Some(82.0),
        load: Some([2.1, 1.8, 1.6]),
        memory_total: 16 * GB,
        memory_available: 16 * GB * 29 / 100,
        disk: Some((926 * GB, 926 * GB * 7 / 100)),
    }
}

async fn app_with_machine(server: &Server) -> App {
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    // The real server answers for its own directory (the stabilized one does not
    // exist); its numbers change from run to run, so the snapshots use the fixed sample
    // after checking the shape.
    let shown = std::mem::replace(
        &mut app.session.directory,
        server.root().display().to_string(),
    );
    server
        .perform(&mut app, vec![Action::LoadSidebar(Part::Machine)])
        .await;
    app.session.directory = shown;
    let live = app.sidebar.machine.ready().expect("GET /system answered");
    assert!(live.cores >= 1 && live.memory_total > 0, "{live:?}");
    assert!(live.disk.is_some(), "the Location's disk: {live:?}");
    app.sidebar.apply(Loaded::Machine(Source::Ready(sample())));
    app
}

#[tokio::test]
async fn machine_bars_render_in_light_dark_and_no_color() {
    let server = Server::start().await;
    let mut app = app_with_machine(&server).await;
    for (name, opts) in [
        (
            "machine_light_160x48",
            Opts {
                light: true,
                legend: &["CPU", "Mem", "Disk"],
                ..Opts::default()
            },
        ),
        (
            "machine_dark_160x48",
            Opts {
                legend: &["CPU", "Mem", "Disk"],
                ..Opts::default()
            },
        ),
        (
            "machine_no_color_160x48",
            Opts {
                no_color: true,
                ..Opts::default()
            },
        ),
    ] {
        let screen = render(&mut app, 160, 48, &opts);
        assert!(screen.contains("▼ Machine"), "{screen}");
        assert!(screen.contains("12 cores · 82% idle"), "{screen}");
        assert!(screen.contains("11.4 / 16.0 GB · 4.6 free"), "{screen}");
        assert!(screen.contains("861 / 926 GB · 64.8 free"), "{screen}");
        assert!(screen.contains("load 2.1 1.8 1.6"), "{screen}");
        if opts.no_color {
            assert!(screen.contains("[##"), "{screen}");
        }
        insta::assert_snapshot!(name, screen);
    }
}

#[tokio::test]
async fn bar_colors_follow_the_70_and_90_percent_thresholds() {
    let server = Server::start().await;
    let mut app = app_with_machine(&server).await;
    let t = app.theme.clone();
    for (percent, color) in [
        (69.0, t.success),
        (70.0, t.warning),
        (89.0, t.warning),
        (90.0, t.error),
    ] {
        assert_eq!(
            crate::view::sidebar::machine_color(&t, percent),
            color,
            "{percent}"
        );
    }
    // The drawn bars: CPU 18% busy, memory 71% and disk 93% in use.
    let buf = buffer(&mut app, 160, 48, &Opts::default());
    for (label, color) in [("CPU ", t.success), ("Mem ", t.warning), ("Disk ", t.error)] {
        let y = row_of(&buf, label);
        let x = (0..buf.area.width)
            .find(|&x| buf[(x, y)].symbol() == "█")
            .unwrap_or_else(|| panic!("{label} has a bar"));
        assert_eq!(buf[(x, y)].fg, color, "{label}");
        assert_eq!(buf[(x, y)].bg, t.border, "{label} track");
    }
}

#[tokio::test]
async fn polling_runs_only_while_the_sidebar_shows_and_the_terminal_is_focused() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let start = app.started();
    let at = |app: &mut App, ms: u64| app.clock = Some(start + Duration::from_millis(ms));
    let machine = vec![Action::LoadSidebar(Part::Machine)];

    // Docked at 160 columns: asks at once, then every 3 s.
    buffer(&mut app, 160, 48, &Opts::default());
    at(&mut app, 0);
    assert_eq!(app.machine_poll(), machine);
    at(&mut app, 2_999);
    assert!(app.machine_poll().is_empty());
    at(&mut app, 3_000);
    assert_eq!(app.machine_poll(), machine);

    // Unfocused: nothing.
    app.focused = false;
    at(&mut app, 9_000);
    assert!(app.machine_poll().is_empty());
    app.focused = true;

    // Hidden at 100 columns without the drawer: nothing.
    buffer(&mut app, 100, 30, &Opts::default());
    at(&mut app, 12_000);
    assert!(app.machine_poll().is_empty());

    // `tui.machine_stats: off`: nothing, and the section is not drawn.
    buffer(&mut app, 160, 48, &Opts::default());
    app.prefs
        .set("machine_stats", "off", crate::prefs::Origin::Saved);
    at(&mut app, 15_000);
    assert!(app.machine_poll().is_empty());
    app.sidebar.apply(Loaded::Machine(Source::Ready(sample())));
    let screen = render(&mut app, 160, 48, &Opts::default());
    assert!(!screen.contains("Machine"), "{screen}");
}

#[tokio::test]
async fn a_failed_read_before_any_sample_shows_a_dash() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    app.sidebar.apply(Loaded::MachineFailed);
    let screen = render(&mut app, 160, 48, &Opts::default());
    let header = screen
        .lines()
        .position(|l| l.contains("▼ Machine"))
        .unwrap();
    assert!(
        screen.lines().nth(header + 1).unwrap().contains('—'),
        "{screen}"
    );
    // A later failure keeps the last sample.
    app.sidebar.apply(Loaded::Machine(Source::Ready(sample())));
    app.sidebar.apply(Loaded::MachineFailed);
    assert!(render(&mut app, 160, 48, &Opts::default()).contains("82% idle"));
}

/// A server of another build lacks `GET /system`: the section says to restart it, the
/// sidebar warns once, and the same server's own build shows nothing of the kind.
#[tokio::test]
async fn an_older_server_without_the_route_asks_for_a_restart() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    server
        .perform(&mut app, vec![Action::LoadSidebar(Part::Health)])
        .await;
    assert!(!app.sidebar.server_stale(), "the test server is this build");
    app.sidebar.apply(Loaded::Machine(Source::Missing));
    let current = render(&mut app, 160, 48, &Opts::default());
    assert!(!current.contains("▼ Machine"), "{current}");
    assert!(!current.contains("older server build"), "{current}");

    app.sidebar.apply(Loaded::Version(
        Some("0.1.0".into()),
        Some("older-build".into()),
    ));
    assert!(app.sidebar.server_stale());
    let screen = render(&mut app, 160, 48, &Opts::default());
    assert!(screen.contains("▼ Machine"), "{screen}");
    assert!(screen.contains(crate::perform::NEWER_SERVER), "{screen}");
    assert!(
        screen.contains("⚠ older server build · cyber service restart"),
        "{screen}"
    );
    // A server that predates build identities is older too.
    app.sidebar
        .apply(Loaded::Version(Some("0.1.0".into()), None));
    assert!(app.sidebar.server_stale());
    let buf = buffer(&mut app, 160, 48, &Opts::default());
    let y = row_of(&buf, crate::perform::NEWER_SERVER);
    let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
    let column = line.find("needs").expect("hint on the row");
    let x = line[..column].chars().count() as u16;
    assert_eq!(buf[(x, y)].fg, app.theme.warning);
}
