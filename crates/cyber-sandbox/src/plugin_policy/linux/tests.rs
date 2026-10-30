use super::*;
fn policy<'a>(private: &'a [MaskPath<'a>]) -> LinuxPolicy<'a> {
    LinuxPolicy {
        helper: Path::new("/owned/helper"),
        target: Path::new("/project/plugins/level/package/main"),
        profile: Path::new("/owned/profile"),
        package: Path::new("/project/plugins/level/package"),
        scratch: Path::new("/owned/scratch"),
        location: Path::new("/project"),
        mask_file: Path::new("/owned/empty"),
        bootstrap: &[],
        private,
        read_only: &[],
        target_args: &[],
        fs_read: false,
        fs_write: true,
        shell: false,
        namespace_proc: false,
    }
}
fn position(args: &[String], group: &[&str]) -> usize {
    args.windows(group.len())
        .position(|parts| parts.iter().map(String::as_str).eq(group.iter().copied()))
        .unwrap()
}
#[test]
fn linux_layers_order_anchors_immutable_mounts_masks_and_scratch() {
    let private = [
        MaskPath::Directory(Path::new("/project/private/level/state")),
        MaskPath::File(Path::new("/project/keys/level/key")),
    ];
    let plan = compose(&policy(&private)).unwrap();
    let anchor = position(
        &plan.arguments,
        &["--bind", "/project/private", "/project/private"],
    );
    let deep = position(
        &plan.arguments,
        &["--bind", "/project/private/level", "/project/private/level"],
    );
    let immutable = position(
        &plan.arguments,
        &[
            "--ro-bind",
            "/project/plugins/level/package",
            "/project/plugins/level/package",
        ],
    );
    let mask = position(
        &plan.arguments,
        &["--tmpfs", "/project/private/level/state"],
    );
    let scratch = position(
        &plan.arguments,
        &["--bind", "/owned/scratch", "/owned/scratch"],
    );
    assert!(anchor < deep && deep < immutable && immutable < mask && mask < scratch);
    assert!(!plan.landlock.contains("R\t/project\n"));
    assert!(plan.landlock.contains("W\t/project\n"));
    assert!(plan.landlock.contains("R\t/owned/scratch\n"));
    assert!(plan.arguments.contains(&"--initial-shell=0".into()));
}
#[test]
fn linux_proc_is_fresh_and_never_replaced_by_a_host_bind() {
    let mut p = policy(&[]);
    p.namespace_proc = true;
    let plan = compose(&p).unwrap();
    position(&plan.arguments, &["--proc", "/proc"]);
    position(&plan.arguments, &["--tmpfs", "/proc/1"]);
    position(&plan.arguments, &["--tmpfs", "/proc/sys"]);
    assert!(
        !plan
            .arguments
            .windows(3)
            .any(|p| p == ["--ro-bind", "/proc", "/proc"])
    );
    assert!(plan.landlock.contains("R\t/proc\n"));
    let bootstrap = [Path::new("/proc")];
    p.bootstrap = &bootstrap;
    assert_eq!(compose(&p), Err(Error::Input));
}
#[test]
fn linux_inputs_refuse_unsafe_overlap_injection_and_argument_overflow() {
    let mut p = policy(&[]);
    for target in [
        "/",
        "relative",
        "/owned/../main",
        "/owned/main\nW\t/project",
    ] {
        p.target = Path::new(target);
        assert_eq!(compose(&p), Err(Error::Input));
    }
    p.target = Path::new("/project/plugins/level/package/main");
    p.scratch = p.package;
    assert_eq!(compose(&p), Err(Error::Overlap));
    p.scratch = Path::new("/owned/scratch");
    let args = vec!["argument".to_owned(); 252];
    p.target_args = &args;
    assert!(compose(&p).is_ok());
    let args = vec!["argument".to_owned(); 253];
    p.target_args = &args;
    assert_eq!(compose(&p), Err(Error::Limit));
}

#[test]
fn linux_private_parents_cover_children_without_inner_mount_creation() {
    let masks = [
        MaskPath::File(Path::new("/project/private/child/secret")),
        MaskPath::Directory(Path::new("/project/private")),
        MaskPath::Directory(Path::new("/project/private/child")),
    ];
    let plan = compose(&policy(&masks)).unwrap();
    assert_eq!(
        plan.arguments
            .iter()
            .filter(|arg| arg.as_str() == "--tmpfs")
            .count(),
        1
    );
    assert!(
        !plan
            .arguments
            .contains(&"/project/private/child/secret".into())
    );
}

#[test]
fn linux_conflicting_private_kinds_and_file_ancestors_are_refused() {
    let masks = [
        MaskPath::File(Path::new("/project/private")),
        MaskPath::Directory(Path::new("/project/private")),
    ];
    assert_eq!(compose(&policy(&masks)), Err(Error::Input));
    let masks = [
        MaskPath::File(Path::new("/project/private")),
        MaskPath::File(Path::new("/project/private/child")),
    ];
    assert_eq!(compose(&policy(&masks)), Err(Error::Overlap));
}

#[test]
fn linux_private_masks_can_cover_readonly_configuration_descendants() {
    let masks = [MaskPath::Directory(Path::new("/project/config/private"))];
    let mut p = policy(&masks);
    let readonly = [Path::new("/project/config")];
    p.read_only = &readonly;
    let plan = compose(&p).unwrap();
    assert!(
        position(
            &plan.arguments,
            &["--ro-bind", "/project/config", "/project/config"]
        ) < position(&plan.arguments, &["--tmpfs", "/project/config/private"])
    );
}
