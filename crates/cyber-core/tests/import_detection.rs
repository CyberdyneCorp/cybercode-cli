use cyber_core::import::{SourceRoots, SourceTool, detect_sources};
fn write(root: &std::path::Path, path: &str, text: &str) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}
#[test]
fn detection_parses_three_source_shapes_and_reports_raw_counts_without_values() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let project = root.join("repo");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    write(&project, ".claude/agents/review.md", "private-frontmatter");
    write(&project, ".claude/commands/test.md", "private-command");
    write(&project, ".claude/skills/check/SKILL.md", "private-skill");
    write(
        &project,
        ".claude/settings.json",
        r#"{"env":{"TOKEN":"private-secret"},"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"do-not-run"},{"type":"prompt","prompt":"private-prompt"}]}]}}"#,
    );
    write(
        &project,
        ".codex/config.toml",
        "[agents]\nmax_threads=4\n[agents.review]\nconfig_file='not-resolved.toml'\n[mcp_servers.first]\ncommand='do-not-run'\n[mcp_servers.second]\nurl='private-url'\n",
    );
    write(
        &project,
        "opencode.jsonc",
        r#"{//comment
      "agents":{"build":{}},"command":{"test":{}},"mcp":{"servers":{"one":{"enabled":false}}},
    }"#,
    );
    let report = detect_sources(&SourceRoots {
        project_root: project.clone(),
        directory: project.clone(),
        home,
        codex_home: None,
    })
    .unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let observed: Vec<_> = report
        .tools
        .iter()
        .map(|t| {
            let c = &t.counts;
            (
                t.tool,
                c.agents,
                c.commands,
                c.skills,
                c.mcp_servers,
                c.hooks,
            )
        })
        .collect();
    assert_eq!(
        observed,
        vec![
            (SourceTool::OpenCode, 1, 1, 0, 1, 0),
            (SourceTool::Codex, 1, 0, 0, 2, 0),
            (SourceTool::Claude, 1, 1, 1, 0, 2),
        ]
    );
    assert!(!report.complete);
    assert_eq!(
        report
            .tools
            .iter()
            .map(|t| (t.counts_complete, t.counts.sessions, t.read_time_coverage))
            .collect::<Vec<_>>(),
        vec![(false, None, None); 3]
    );
    assert!(!serde_json::to_string(&report).unwrap().contains("private-"));
    assert!(!project.join(".cyber").exists());
}
#[test]
fn malformed_configuration_is_an_issue_and_never_leaks_parser_values() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, ".claude/settings.json", "private-secret");
    write(&root, ".codex/config.toml", "private-secret = [oops");
    write(
        &root,
        "opencode.json",
        r#"{"agents":{"a":{}},"commands":false}"#,
    );
    let report = detect_sources(&SourceRoots {
        project_root: root.clone(),
        directory: root.clone(),
        home: root.clone(),
        codex_home: None,
    })
    .unwrap();
    assert!(report.issues.len() >= 3);
    assert!(report.tools.iter().all(|t| t.counts.agents == 0));
    assert!(!format!("{report:?}").contains("private-secret"));
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("private-secret")
    );
}

#[test]
fn detection_bounds_aggregate_parsed_source_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let project = root.join("repo");
    let home = root.join("home");
    std::fs::create_dir_all(&project).unwrap();
    let comment = format!("#{}", "x".repeat(1024 * 1024 - 1));
    for index in 0..17 {
        write(&home, &format!(".codex/{index}.config.toml"), &comment);
    }
    let report = detect_sources(&SourceRoots {
        project_root: project.clone(),
        directory: project,
        home,
        codex_home: None,
    })
    .unwrap();
    assert_eq!(report.tools[0].files.len(), 17);
    assert_eq!(report.issues.len(), 1);
    assert!(report.issues[0].reason.contains("sixteen MiB"));
}
