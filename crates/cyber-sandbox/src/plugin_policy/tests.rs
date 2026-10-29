use super::*;

fn profile<'a>(bootstrap: &'a [ReadPath<'a>], private: &'a [&'a Path]) -> MacProfile<'a> {
    MacProfile {
        target: Path::new("/runtime/bin/host"),
        helper_identifier: "cyber.helper.fresh-fixture",
        package: Path::new("/owned/package"),
        scratch: Path::new("/owned/scratch"),
        location: Path::new("/project"),
        bootstrap,
        private,
        read_only: &[],
        fs_read: true,
        fs_write: true,
        shell: false,
    }
}

#[cfg(target_os = "macos")]
#[test]
fn native_profile_denials_follow_workspace_grants_and_protect_every_ancestor() {
    let bootstrap = [ReadPath::Directory(Path::new("/runtime/lib"))];
    let private = [Path::new("/project/nested/private")];
    let text = profile(&bootstrap, &private).render().unwrap();
    let allow = text
        .find("(allow file-write* (subpath \"/project\"))")
        .unwrap();
    for denial in [
        "(deny file-write* (subpath \"/owned/package\"))",
        "(deny file-write* (literal \"/runtime/bin/host\"))",
        "(deny file-read* file-write* (subpath \"/project/nested/private\"))",
        "(deny file-write-unlink (literal \"/project/nested\"))",
        "(deny file-write-unlink (literal \"/runtime/bin\"))",
    ] {
        assert!(text.find(denial).unwrap() > allow, "{denial}");
    }
    assert!(!text.contains("(allow process-fork)"));
    assert!(!text.contains("(allow process-exec)\n"));
}

#[cfg(target_os = "macos")]
#[test]
fn native_profile_refuses_invalid_paths_identifiers_overlap_and_root_budgets() {
    let mut plan = profile(&[], &[]);
    for path in ["relative", "/", "/runtime/../target", "/target\n(injected)"] {
        plan.target = Path::new(path);
        assert_eq!(plan.render(), Err(Error::Input));
    }
    plan.target = Path::new("/runtime/bin/host");
    plan.helper_identifier = "bad\"identifier";
    assert_eq!(plan.render(), Err(Error::Input));
    plan.helper_identifier = "cyber.helper.fixture";
    plan.scratch = Path::new("/owned/package/scratch");
    assert_eq!(plan.render(), Err(Error::Overlap));
    plan.scratch = Path::new("/owned/scratch");
    let bootstrap = vec![ReadPath::Directory(Path::new("/runtime/lib")); 65];
    plan.bootstrap = &bootstrap;
    assert_eq!(plan.render(), Err(Error::Limit));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn native_profile_never_silently_supplies_a_policy_for_another_platform() {
    assert_eq!(profile(&[], &[]).render(), Err(Error::Unavailable));
}

#[cfg(target_os = "macos")]
#[test]
fn native_profile_accepts_root_limit_and_refuses_rendered_policy_overflow() {
    let bootstrap = vec![ReadPath::Directory(Path::new("/runtime/lib")); 60];
    let mut plan = profile(&bootstrap, &[]);
    assert!(plan.render().is_ok());
    let paths: Vec<_> = (0..16)
        .map(|i| {
            let mut path = std::path::PathBuf::from(format!("/large-{i}"));
            for _ in 0..30 {
                path.push("component-with-some-length");
            }
            path
        })
        .collect();
    let bootstrap: Vec<_> = paths.iter().map(|path| ReadPath::Directory(path)).collect();
    plan.bootstrap = &bootstrap;
    assert_eq!(plan.render(), Err(Error::Limit));
}
