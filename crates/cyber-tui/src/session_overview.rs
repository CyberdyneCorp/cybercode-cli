//! An unfiltered, bounded projection of the shared agent directory.
use crate::agents::{Directory, Entry, Kind, Location, State};
use std::collections::BTreeSet;

pub const LIMIT: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Local,
    Team,
    Remote,
}
impl Group {
    pub const ALL: [Self; 3] = [Self::Local, Self::Team, Self::Remote];
    pub fn title(self) -> &'static str {
        match self {
            Self::Local => "Local Sessions",
            Self::Team => "Team / child agents",
            Self::Remote => "Remote",
        }
    }
}

pub struct Overview<'a> {
    pub entries: Vec<&'a Entry>,
    pub counts: [usize; 3],
    pub total: usize,
    pub unavailable_peers: usize,
}

pub fn group(entry: &Entry) -> Group {
    match (&entry.kind, &entry.location) {
        (Kind::Peer | Kind::HubAgent, _) | (_, Location::Peer { .. } | Location::Hub(_)) => {
            Group::Remote
        }
        (Kind::Subagent | Kind::WorkflowAgent | Kind::Teammate, _) => Group::Team,
        _ => Group::Local,
    }
}

fn rank(state: State) -> u8 {
    match state {
        State::Running | State::WaitingInput => 0,
        State::Online => 1,
        State::Idle => 2,
        State::Offline => 3,
        State::Unknown => 4,
    }
}

pub fn name(entry: &Entry) -> &str {
    if entry.name.is_empty() {
        &entry.address
    } else {
        &entry.name
    }
}

pub fn detail(entry: &Entry) -> String {
    let place = match &entry.location {
        Location::Peer { peer, .. } => format!("peer:{peer}"),
        Location::Hub(project) => format!("hub:{project}"),
        Location::Local(_) => entry.kind.label().to_owned(),
    };
    format!("{} · {place}", entry.state.label())
}

fn wildcard(entry: &Entry) -> bool {
    matches!(entry.kind, Kind::Peer)
        && (entry.address.starts_with("*@")
            || entry.target.as_deref().is_some_and(|v| v.starts_with("*@"))
            || entry.key.starts_with("listing:*@"))
}

fn named(entry: &Entry, current: &str) -> bool {
    !matches!(entry.kind, Kind::HubMember)
        && !entry.key.starts_with("invalid-")
        && !wildcard(entry)
        && entry.session_id.as_deref() != Some(current)
        && !name(entry).is_empty()
        && match &entry.kind {
            Kind::Peer => entry
                .key
                .strip_prefix("listing:")
                .is_some_and(|id| id.starts_with("ses_") && id.contains('@')),
            Kind::HubAgent => entry
                .participant
                .as_ref()
                .and_then(|p| p["id"].as_str())
                .is_some_and(|id| !id.is_empty() && crate::messaging::clean(id) == id),
            _ => entry.session_id.is_some(),
        }
}

pub fn project<'a>(directory: &'a Directory, current: &str) -> Overview<'a> {
    project_limit(directory, current, LIMIT)
}

pub fn project_limit<'a>(directory: &'a Directory, current: &str, limit: usize) -> Overview<'a> {
    let limit = limit.min(LIMIT);
    let mut seen = BTreeSet::new();
    let mut entries: Vec<_> = directory
        .rows()
        .filter(|entry| named(entry, current) && seen.insert(entry.key.clone()))
        .collect();
    entries.sort_by(|a, b| {
        (group(a), rank(a.state), name(a).to_lowercase(), &a.key).cmp(&(
            group(b),
            rank(b.state),
            name(b).to_lowercase(),
            &b.key,
        ))
    });
    let mut counts = [0; 3];
    for entry in &entries {
        counts[group(entry) as usize] += 1;
    }
    let total = entries.len();
    // Reserve one name per source group, then fill the remaining budget in
    // display order. Local history must not crowd all collaborators out.
    let mut chosen = BTreeSet::new();
    for wanted in Group::ALL {
        if chosen.len() >= limit {
            break;
        }
        if let Some(entry) = entries.iter().find(|entry| group(entry) == wanted) {
            chosen.insert(entry.key.as_str());
        }
    }
    for entry in &entries {
        if chosen.len() >= limit {
            break;
        }
        chosen.insert(entry.key.as_str());
    }
    entries.retain(|entry| chosen.contains(entry.key.as_str()));
    Overview {
        entries,
        counts,
        total,
        unavailable_peers: directory
            .rows()
            .filter(|entry| wildcard(entry))
            .map(|entry| entry.key.clone())
            .collect::<BTreeSet<_>>()
            .len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(id: &str, title: &str, kind: &str) -> Entry {
        Entry::from_listing(&json!({"session_id":id,"address":id,"name":title,
            "kind":kind,"state":"idle","location":{"directory":"/repo"}}))
    }

    #[test]
    fn fair_bound_keeps_names_from_every_group_and_truthful_counts() {
        let mut directory = Directory::default();
        directory.messaging.rows = (0..12)
            .map(|n| entry(&format!("ses_{n}"), &format!("Local {n:02}"), "local"))
            .collect();
        directory.messaging.rows.extend([
            entry("ses_team", "Check regression tests", "subagent"),
            entry("ses_remote@studio", "Local 00", "remote"),
        ]);
        let overview = project(&directory, "ses_me");
        assert_eq!(overview.total, 14);
        assert_eq!(overview.counts, [12, 1, 1]);
        assert_eq!(overview.entries.len(), LIMIT);
        for wanted in Group::ALL {
            assert!(overview.entries.iter().any(|entry| group(entry) == wanted));
        }
        assert_eq!(
            overview
                .entries
                .iter()
                .filter(|e| name(e) == "Local 00")
                .count(),
            2
        );
        assert_eq!(
            detail(overview.entries.last().unwrap()),
            "idle · peer:studio"
        );
        directory.filter = "does not match".into();
        directory.kind = crate::agents::KindFilter::Hub;
        assert_eq!(
            project(&directory, "ses_me").total,
            14,
            "directory filters are isolated"
        );
    }

    #[test]
    fn current_wildcard_humans_and_unsafe_ids_are_not_named_sessions() {
        let mut directory = Directory::default();
        let mut human = entry("ses_human", "Human", "local");
        human.kind = Kind::HubMember;
        directory.messaging.rows = vec![
            entry("ses_me", "Current", "local"),
            entry("*@offline", "offline", "peer"),
            entry("ses_bad\n", "Unsafe", "local"),
            entry("ses_good", "", "local"),
            human,
        ];
        let overview = project(&directory, "ses_me");
        assert_eq!(overview.total, 1);
        assert_eq!(overview.unavailable_peers, 1);
        assert_eq!(name(overview.entries[0]), "ses_good");
        assert_ne!(
            entry("ses_bad\n", "A", "local").key,
            entry("ses_bad", "B", "local").key
        );
    }

    #[test]
    fn running_first_then_unicode_names_and_stable_identity() {
        let mut directory = Directory::default();
        let mut running = entry("ses_z", "工作", "local");
        running.state = State::Running;
        directory.messaging.rows = vec![
            entry("ses_b", "Same", "local"),
            running,
            entry("ses_a", "Same", "local"),
        ];
        let keys: Vec<_> = project(&directory, "ses_me")
            .entries
            .into_iter()
            .map(|e| e.key.as_str())
            .collect();
        assert_eq!(keys, ["listing:ses_z", "listing:ses_a", "listing:ses_b"]);
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::{app::App, sidebar::Section};
    use serde_json::json;

    #[test]
    fn six_names_keep_remote_provenance_and_directory_footer_on_short_layouts() {
        let mut app = App::new(Default::default(), Vec::new(), "cyber");
        app.session.id = "ses_ME".into();
        app.session.title = "A realistic current Session title".into();
        app.directory.messaging.rows = (0..6).map(|n| {
            let (id,kind) = match n { 4 => ("ses_team".to_owned(),"subagent"), 5 => ("ses_remote@studio".to_owned(),"remote"), _ => (format!("ses_{n}"),"local") };
            Entry::from_listing(&json!({"session_id":id,"address":id,"name":format!("Named work {n}"),"kind":kind,"state":"running"}))
        }).collect();
        app.refresh_reachable_rows();
        for (width, height, drawer) in [(120, 34, false), (100, 30, true)] {
            app.sidebar.drawer = drawer;
            let screen = crate::screens::render(&mut app, width, height, &Default::default());
            assert!(screen.contains("peer:studio"), "{screen}");
            assert!(screen.contains("/agents"), "{screen}");
        }
    }

    #[test]
    fn named_overview_is_visible_in_dock_and_drawer_and_collapses_cleanly() {
        let mut app = App::new(Default::default(), Vec::new(), "cyber");
        app.session.id = "ses_ME".into();
        app.directory.messaging.rows = [
            ("ses_local", "Update README examples", "local"),
            ("ses_child", "Check regression tests", "subagent"),
            ("ses_remote@studio", "Review API contracts", "remote"),
        ]
        .into_iter()
        .map(|(id, name, kind)| {
            Entry::from_listing(&json!({
                "session_id":id,"address":id,"name":name,"kind":kind,"state":"running"
            }))
        })
        .collect();
        app.refresh_reachable_rows();
        for (width, height, drawer) in [(160, 48, false), (120, 40, false), (100, 40, true)] {
            app.sidebar.drawer = drawer;
            let screen = crate::screens::render(&mut app, width, height, &Default::default());
            assert!(screen.contains("Sessions & agents"), "{screen}");
            assert!(screen.contains("Update README examples"), "{screen}");
            assert!(screen.contains("Check regression tests"), "{screen}");
            assert!(screen.contains("peer:studio"), "{screen}");
            assert!(screen.contains("Open /agents"), "{screen}");
            insta::assert_snapshot!(format!("session_overview_{width}x{height}"), screen);
        }
        for (label, opts) in [
            (
                "light",
                crate::screens::Opts {
                    light: true,
                    legend: &["Sessions & agents", "peer:studio"],
                    ..Default::default()
                },
            ),
            (
                "no_color",
                crate::screens::Opts {
                    no_color: true,
                    legend: &["Sessions & agents", "peer:studio"],
                    ..Default::default()
                },
            ),
        ] {
            app.sidebar.drawer = false;
            let screen = crate::screens::render(&mut app, 160, 48, &opts);
            assert!(screen.contains("peer:studio"));
            insta::assert_snapshot!(format!("session_overview_{label}"), screen);
        }
        app.sidebar.collapsed.insert(Section::Sessions);
        let screen = crate::screens::render(&mut app, 160, 48, &Default::default());
        assert!(screen.contains("Sessions & agents"));
        assert!(!screen.contains("Check regression tests"));
        assert!(!app.sidebar_overview_demand());
    }
}
