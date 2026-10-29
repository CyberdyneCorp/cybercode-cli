//! Actual packaged helper with the production mount and Landlock composer.
use super::*;
use cyber_sandbox::plugin_policy::{LinuxPolicy, MaskPath};
use std::os::unix::fs::PermissionsExt;

struct Composed {
    fixture: Fixture,
    project: PathBuf,
    package: PathBuf,
    target: PathBuf,
    private_dir: PathBuf,
    private_file: PathBuf,
    mask: PathBuf,
    bootstrap: Vec<PathBuf>,
    namespace_proc: bool,
}
impl Composed {
    fn new() -> Self {
        let fixture = Fixture::new();
        let project = fixture.root.join("project \"quote\" \\ workspace");
        let package = project.join("extensions/level/package");
        let private_dir = project.join("owner/level/state");
        let private_file = project.join("keys/level/secret");
        for directory in [
            package.as_path(),
            private_dir.as_path(),
            private_file.parent().unwrap(),
        ] {
            std::fs::create_dir_all(directory).unwrap();
        }
        for file in [
            package.join("data"),
            project.join("data"),
            private_dir.join("secret"),
            private_file.clone(),
        ] {
            std::fs::write(file, b"owned-fixture").unwrap();
        }
        symlink(&private_file, package.join("alias")).unwrap();
        let target = package.join("main");
        std::fs::copy(&fixture.payload, &target).unwrap();
        let mask = fixture.root.join("empty-mask");
        std::fs::write(&mask, b"").unwrap();
        std::fs::set_permissions(&mask, std::fs::Permissions::from_mode(0o000)).unwrap();
        let bootstrap = ["/usr", "/lib", "/lib64", "/bin", "/etc/ld.so.cache"]
            .into_iter()
            .filter(|path| Path::new(path).exists())
            .map(|path| std::fs::canonicalize(path).unwrap())
            .collect();
        Self {
            fixture,
            project,
            package,
            target,
            private_dir,
            private_file,
            mask,
            bootstrap,
            namespace_proc: false,
        }
    }
    fn invoke(
        &self,
        target: &Path,
        args: &[String],
        read: bool,
        write: bool,
        shell: bool,
    ) -> Output {
        let helper = Path::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"));
        let profile = self.fixture.root.join("composed-profile");
        let scratch = self.fixture.root.join("scratch");
        let bootstrap: Vec<_> = self.bootstrap.iter().map(PathBuf::as_path).collect();
        let masks = [
            MaskPath::Directory(&self.private_dir),
            MaskPath::File(&self.private_file),
        ];
        let policy = LinuxPolicy {
            helper,
            target,
            profile: &profile,
            package: &self.package,
            scratch: &scratch,
            location: &self.project,
            mask_file: &self.mask,
            bootstrap: &bootstrap,
            private: &masks,
            read_only: &[],
            target_args: args,
            fs_read: read,
            fs_write: write,
            shell,
            namespace_proc: self.namespace_proc,
        };
        let plan = policy.compose().unwrap();
        std::fs::write(&profile, &plan.landlock).unwrap();
        let mut command = Command::new("/usr/bin/bwrap");
        command
            .args(&plan.arguments)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("CYBER_PROOF_PRIVATE", &self.private_file);
        success(&mut command, Some(b"{\"jsonrpc\":\"2.0\",\"id\":\"native-proof\",\"method\":\"tool/execute\",\"params\":{\"name\":\"native-probe\",\"input\":{}}}\n"))
    }
    fn inspect(&self, read: bool, write: bool, shell: bool) -> Value {
        let scratch = self.fixture.root.join("scratch");
        let mut args = vec!["inspect".into()];
        for path in [
            self.package.join("data"),
            self.project.join("data"),
            self.private_file.clone(),
            scratch.join("roundtrip"),
            self.project.join("written"),
            self.package.join("alias"),
            self.private_dir.join("secret"),
            scratch,
            self.project.clone(),
        ] {
            args.push(path.to_str().unwrap().into());
        }
        args.extend(["0".into(), "0".into(), "1".into()]);
        let result = self.invoke(&self.target, &args, read, write, shell);
        let packet: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(packet["jsonrpc"], "2.0");
        assert_eq!(packet["id"], "native-proof");
        assert!(packet.get("error").is_none());
        packet["result"].clone()
    }
}
fn capabilities(value: Value, read: bool, write: bool, shell: bool) {
    for key in [
        "stdio_tool",
        "package_read",
        "thread",
        "scratch_write",
        "scratch_read",
        "cannot_relax",
        "filter_replacement_denied",
    ] {
        assert_eq!(value[key], true, "{key}: {value}");
    }
    for key in [
        "private_read",
        "private_list",
        "private_write",
        "protected_read",
        "protected_write",
        "symlink_private_read",
        "hardlink_private",
        "package_hardlink",
        "package_alias_write",
        "inherited_descriptor",
        "constructor_private_read",
        "project_rename",
    ] {
        assert_eq!(value[key], false, "{key}: {value}");
    }
    assert_eq!(value["project_read"], read, "project_read: {value}");
    assert_eq!(value["project_list"], read, "{value}");
    assert_eq!(value["project_write"], write, "{value}");
    assert_eq!(value["written_project_read"], read && write, "{value}");
    assert_eq!(
        value["thread_reads"],
        serde_json::json!([read, false]),
        "{value}"
    );
    for key in [
        "shell",
        "native_fork",
        "native_spawn",
        "constructor_fork",
        "thread_shell",
    ] {
        assert_eq!(value[key], shell, "{key}: {value}");
    }
    let children = if shell {
        serde_json::json!([read, false])
    } else {
        Value::Null
    };
    assert_eq!(value["child_reads"], children, "{value}");
    assert_eq!(value["native_child_reads"], children, "{value}");
}
#[test]
fn rust_linux_policy_composes_mounts_with_landlock_and_the_packaged_entry() {
    let f = Composed::new();
    for (read, write, shell) in [
        (false, false, false),
        (false, false, true),
        (false, true, false),
        (false, true, true),
        (true, false, false),
        (true, false, true),
        (true, true, false),
        (true, true, true),
    ] {
        capabilities(f.inspect(read, write, shell), read, write, shell);
        assert_eq!(std::fs::read(&f.private_file).unwrap(), b"owned-fixture");
        assert_eq!(
            std::fs::read(f.private_dir.join("secret")).unwrap(),
            b"owned-fixture"
        );
        assert_eq!(
            std::fs::read(f.package.join("data")).unwrap(),
            b"owned-fixture"
        );
        let written = f.project.join("written");
        if written.exists() {
            std::fs::remove_file(written).unwrap();
        }
    }
}

fn mutation(
    f: &Composed,
    target: &Path,
    operation: &str,
    source: &Path,
    destination: &Path,
) -> Value {
    let args = [
        operation.into(),
        source.to_str().unwrap().into(),
        destination.to_str().unwrap().into(),
    ];
    let result = f.invoke(target, &args, true, true, false);
    serde_json::from_slice(&result.stdout).unwrap()
}
#[test]
fn rust_linux_policy_protects_ancestors_and_preserves_public_renames() {
    let f = Composed::new();
    let target = f.package.join("mutation");
    success(
        Command::new("cc")
            .args(["-Wall", "-Wextra", "-Werror"])
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugin_path_mutation.c"),
            )
            .arg("-o")
            .arg(&target),
        None,
    );
    for root in [&f.package, &f.private_dir, &f.private_file] {
        let parent = root.parent().unwrap();
        for source in [parent, parent.parent().unwrap()] {
            let observed = mutation(
                &f,
                &target,
                "rename",
                source,
                &source.with_extension("moved"),
            );
            assert_eq!(
                observed,
                serde_json::json!({"ok":false,"errno":16}),
                "protected ancestor rename: {source:?}"
            );
            let sibling = source.join("public-sibling");
            std::fs::write(&sibling, b"public-fixture").unwrap();
            assert_eq!(
                mutation(&f, &target, "write", &sibling, &sibling),
                serde_json::json!({"ok":true,"errno":0})
            );
        }
    }
    let data = f.package.join("data");
    assert_eq!(
        mutation(&f, &target, "write", &data, &data),
        serde_json::json!({"ok":false,"errno":30}),
        "immutable package write"
    );
    let public = f.project.join("public");
    std::fs::create_dir(&public).unwrap();
    assert_eq!(
        mutation(
            &f,
            &target,
            "rename",
            &public,
            &public.with_extension("moved")
        ),
        serde_json::json!({"ok":true,"errno":0})
    );
    assert_eq!(std::fs::read(&f.private_file).unwrap(), b"owned-fixture");
    assert_eq!(
        std::fs::read(f.private_dir.join("secret")).unwrap(),
        b"owned-fixture"
    );
}

#[test]
fn rust_linux_policy_private_proc_cannot_restore_host_or_private_file_reads() {
    let mut f = Composed::new();
    f.namespace_proc = true;
    let target = f.package.join("proc-probe");
    success(
        Command::new("cc")
            .args(["-Wall", "-Wextra", "-Werror"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugin_proc_probe.c"))
            .arg("-o")
            .arg(&target),
        None,
    );
    let scratch = f.fixture.root.join("scratch/proc-data");
    std::fs::write(&scratch, b"scratch-fixture").unwrap();
    let args: Vec<String> = [f.project.join("data"), f.private_file.clone(), scratch]
        .iter()
        .map(|path| path.to_str().unwrap().into())
        .collect();
    for read in [false, true] {
        let output = f.invoke(&target, &args, read, false, false);
        let observed: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            observed,
            serde_json::json!({"self":true,"init":false,"sys":false,
            "private_pid_view":true,"project_alias":read,"private_alias":false,"scratch_alias":true}),
            "private proc visibility: {observed}"
        );
    }
}
