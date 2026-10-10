use cyber_core::{
    config::{self, LoadRequest},
    paths::Paths,
    trust::TrustStore,
};
use cyber_tools::lsp::{LaunchOptions, LaunchRequest, LocalLauncher};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

struct Fixture {
    root: tempfile::TempDir,
    repo: PathBuf,
    paths: Paths,
    environment: HashMap<String, String>,
    binary: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let environment = HashMap::from([(
            "CYBER_HOME".into(),
            root.path().join("home").to_string_lossy().into_owned(),
        )]);
        let mut paths = Paths::resolve(&environment, root.path());
        paths.tmp = root.path().join("tmp");
        paths.ensure().unwrap();
        let binary = root.path().join("fixture-server.exe");
        std::fs::write(&binary, "fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let fixture = Self {
            root,
            repo,
            paths,
            environment,
            binary,
        };
        fixture.global(json!({"lsp":{"fixture":{"command":[fixture.binary],"extensions":[".rs"]}},"sandbox":{"network":"off"}}));
        fixture
    }
    fn global(&self, value: Value) {
        std::fs::write(self.paths.config.join("cyber.jsonc"), value.to_string()).unwrap();
    }
    fn launcher(&self) -> Arc<LocalLauncher> {
        Arc::new(
            LocalLauncher::new(
                &self.repo,
                LaunchOptions {
                    checkout_claim: None,
                    paths: self.paths.clone(),
                    home: self.root.path().into(),
                    environment: self.environment.clone(),
                    profile: None,
                    overrides: vec![],
                    flags: json!({}),
                    sandbox_policy: None,
                    helper: cyber_sandbox::find_helper(),
                    credential_env_names: vec![],
                },
            )
            .unwrap(),
        )
    }
    fn request(&self, launcher: &LocalLauncher) -> LaunchRequest {
        LaunchRequest {
            checkouts: vec![],
            location: self.repo.canonicalize().unwrap(),
            root: self.repo.canonicalize().unwrap(),
            server: launcher
                .servers()
                .unwrap()
                .into_iter()
                .find(|s| s.definition.id == "fixture")
                .unwrap(),
        }
    }
}

#[tokio::test]
async fn changed_or_forged_definitions_and_external_roots_are_refused_before_native_spawn() {
    let fixture = Fixture::new();
    let launcher = fixture.launcher();
    let request = fixture.request(&launcher);
    let mut forged = request.clone();
    forged
        .server
        .definition
        .env
        .insert("INJECTED".into(), "private-fixture-secret".into());
    let callback = launcher.callback();
    assert!(callback(forged, CancellationToken::new()).await.is_err());
    let mut outside = request.clone();
    outside.root = fixture.root.path().canonicalize().unwrap();
    assert!(callback(outside, CancellationToken::new()).await.is_err());
    fixture.global(json!({"lsp":false}));
    let error = match callback(request, CancellationToken::new()).await {
        Ok(_) => panic!("unexpected spawn"),
        Err(error) => error,
    };
    assert!(error.acknowledged);
    assert!(!error.to_string().contains("private-fixture-secret"));
    assert_eq!(std::fs::read_dir(&fixture.paths.tmp).unwrap().count(), 0);
}

#[tokio::test]
async fn revoked_checkout_trust_cannot_reuse_an_earlier_server_selection() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.repo.join("cyber.jsonc"),
        json!({"lsp":{"fixture":{"env":{"GOOD":"project-approved"}}}}).to_string(),
    )
    .unwrap();
    let load = || {
        config::load(&LoadRequest {
            location: &fixture.repo,
            paths: &fixture.paths,
            env: &fixture.environment,
            home: fixture.root.path(),
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap()
    };
    let trust = TrustStore::new(fixture.paths.trust_file());
    let report = load().trust;
    trust
        .approve(&report.checkout_root, report.digest.as_deref().unwrap())
        .unwrap();
    let launcher = fixture.launcher();
    let request = fixture.request(&launcher);
    assert_eq!(request.server.definition.env["GOOD"], "project-approved");
    trust.revoke(&report.checkout_root).unwrap();
    let error = match launcher.callback()(request, CancellationToken::new()).await {
        Ok(_) => panic!("unexpected spawn"),
        Err(error) => error,
    };
    assert!(error.acknowledged);
    assert_eq!(std::fs::read_dir(&fixture.paths.tmp).unwrap().count(), 0);
}

#[tokio::test]
async fn cancelled_launch_does_not_prepare_scratch_or_native_effects() {
    let fixture = Fixture::new();
    let launcher = fixture.launcher();
    let request = fixture.request(&launcher);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = match launcher.callback()(request, cancel).await {
        Ok(_) => panic!("unexpected spawn"),
        Err(error) => error,
    };
    assert!(error.acknowledged);
    assert_eq!(std::fs::read_dir(&fixture.paths.tmp).unwrap().count(), 0);
}

#[cfg(all(unix, any(target_os = "macos", target_os = "linux")))]
mod native {
    use super::*;
    use cyber_tools::lsp::Pool;
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    const SCRIPT: &str = r#"#!/usr/bin/env python3
import sys,json,os,pathlib
def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        if line == b'\r\n': break
        key,value = line.decode('ascii').split(':',1)
        if key.lower() == 'content-length': length = int(value)
    return json.loads(sys.stdin.buffer.read(length))
def send(id,result):
    body=json.dumps({'jsonrpc':'2.0','id':id,'result':result}).encode('utf-8')
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n' % len(body)).encode('ascii')+body); sys.stdout.buffer.flush()
def write(path):
    try: pathlib.Path(path).write_text('effect'); return True
    except OSError: return False
while True:
    request = read()
    if request is None: break
    method = request['method']
    if method == 'initialize': send(request['id'],{'capabilities':{}})
    elif method == 'witness': send(request['id'],True)
    elif method in ['textDocument/didOpen','textDocument/didChange']:
        with pathlib.Path('document-events').open('a') as events:
            events.write(json.dumps(request)+'\n')
    elif method == 'probe':
        try: pathlib.Path(sys.argv[2]).read_text(); credentials_readable=True
        except OSError: credentials_readable=False
        send(request['id'],{'outside_written':write(sys.argv[1]),'project_written':write('allowed-write'),'protected_written':write('.git/denied-write'),'credentials_readable':credentials_readable,'provider_present':'CUSTOM_AUTH' in os.environ,'catalog_present':'CATALOG_AUTH' in os.environ,'secret_present':'OPENAI_API_KEY' in os.environ,'good':os.environ.get('GOOD'),'proxy_present':'HTTP_PROXY' in os.environ,'tmp':os.environ.get('TMPDIR'),'cwd':os.getcwd()})
    elif method == 'shutdown': send(request['id'],None)
    elif method == 'exit': break
"#;

    async fn probe(policy: &str) -> Value {
        assert!(
            cyber_sandbox::available(),
            "native sandbox prerequisites are required"
        );
        let mut fixture = Fixture::new();
        let outside = fixture.root.path().join("outside-write");
        let credentials = fixture.root.path().join(".aws/credentials");
        std::fs::create_dir(credentials.parent().unwrap()).unwrap();
        std::fs::write(&credentials, "fixture").unwrap();
        std::fs::write(&fixture.binary, SCRIPT).unwrap();
        std::fs::set_permissions(&fixture.binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        fixture.environment.extend(HashMap::from([
            ("PATH".into(), std::env::var("PATH").unwrap()),
            ("CUSTOM_AUTH".into(), "fixture".into()),
            ("CATALOG_AUTH".into(), "fixture".into()),
            ("OPENAI_API_KEY".into(), "fixture".into()),
            ("http_proxy".into(), "http://bypass.invalid:9".into()),
        ]));
        fixture.global(json!({"lsp":{"fixture":{"command":[fixture.binary,outside,credentials],"extensions":[".rs"],"env":{"GOOD":"configured","HTTP_PROXY":"http://bypass.invalid:9","TMPDIR":"/unowned"}}},"providers":{"fixture":{"env":["CUSTOM_AUTH"]}},"sandbox":{"policy":policy,"network":"off"}}));
        let mut options = LaunchOptions {
            checkout_claim: None,
            paths: fixture.paths.clone(),
            home: fixture.root.path().into(),
            environment: fixture.environment.clone(),
            profile: None,
            overrides: vec![],
            flags: json!({}),
            sandbox_policy: None,
            helper: cyber_sandbox::find_helper(),
            credential_env_names: vec!["CATALOG_AUTH".into()],
        };
        options.home = fixture.root.path().canonicalize().unwrap();
        let launcher = Arc::new(LocalLauncher::new(&fixture.repo, options).unwrap());
        let file = fixture.repo.join("file.rs");
        std::fs::write(&file, "").unwrap();
        let pool = Pool::new(
            &fixture.repo,
            launcher.servers().unwrap(),
            launcher.callback(),
        )
        .unwrap();
        let result = pool
            .ensure("fixture", &file)
            .unwrap()
            .request("probe", json!({}), Duration::from_secs(5))
            .await
            .unwrap();
        assert!(pool.close().await.unwrap()[0].acknowledged);
        assert_eq!(
            result["cwd"],
            fixture
                .repo
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        );
        assert!(
            PathBuf::from(result["tmp"].as_str().unwrap())
                .starts_with(fixture.paths.tmp.canonicalize().unwrap())
        );
        result
    }

    async fn running(
        trusted_project: bool,
    ) -> (Fixture, Pool, cyber_tools::lsp::ServerHandle, PathBuf) {
        assert!(
            cyber_sandbox::available(),
            "native sandbox prerequisites are required"
        );
        let mut fixture = Fixture::new();
        fixture
            .environment
            .insert("PATH".into(), std::env::var("PATH").unwrap());
        std::fs::write(&fixture.binary, SCRIPT).unwrap();
        if trusted_project {
            std::fs::write(
                fixture.repo.join("cyber.jsonc"),
                json!({"lsp":{"fixture":{"env":{"GOOD":"approved-project"}}}}).to_string(),
            )
            .unwrap();
            let report = config::load(&LoadRequest {
                location: &fixture.repo,
                paths: &fixture.paths,
                home: fixture.root.path(),
                env: &fixture.environment,
                profile: None,
                overrides: &[],
                flags: json!({}),
            })
            .unwrap()
            .trust;
            TrustStore::new(fixture.paths.trust_file())
                .approve(&report.checkout_root, report.digest.as_deref().unwrap())
                .unwrap();
        }
        let pool = fixture.launcher().pool().unwrap();
        let file = fixture.repo.join("file.rs");
        std::fs::write(&file, "").unwrap();
        let handle = pool.ensure("fixture", &file).unwrap();
        assert_eq!(
            handle
                .request("witness", json!({}), Duration::from_secs(3))
                .await
                .unwrap(),
            true
        );
        (fixture, pool, handle, file)
    }

    #[tokio::test]
    async fn revoked_running_project_server_cannot_receive_another_document() {
        let (fixture, pool, handle, file) = running(true).await;
        handle
            .open_document(&file, "before-revocation".into())
            .await
            .unwrap();
        handle
            .request("witness", json!({}), Duration::from_secs(3))
            .await
            .unwrap();
        TrustStore::new(fixture.paths.trust_file())
            .revoke(&fixture.repo.canonicalize().unwrap())
            .unwrap();
        assert!(
            handle
                .open_document(&file, "private-after-revocation".into())
                .await
                .is_err()
        );
        assert!(pool.close().await.unwrap()[0].acknowledged);
        let events = std::fs::read_to_string(fixture.repo.join("document-events")).unwrap();
        assert_eq!(events.lines().count(), 1);
        assert!(!events.contains("private-after-revocation"));
        assert_eq!(
            handle.status().status,
            cyber_tools::lsp::ServerState::Broken
        );
    }

    #[tokio::test]
    async fn changed_idle_configuration_fences_and_settles_a_running_root() {
        let (fixture, pool, handle, file) = running(false).await;
        fixture.global(json!({"lsp":false,"sandbox":{"network":"off"}}));
        tokio::time::timeout(Duration::from_secs(3), async {
            while handle.status().status != cyber_tools::lsp::ServerState::Broken {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(pool.close().await.unwrap()[0].acknowledged);
        assert!(pool.ensure("fixture", &file).is_err());
        assert!(!fixture.repo.join("document-events").exists());
    }

    #[tokio::test]
    async fn workspace_policy_launch_is_confined_and_masks_ambient_credentials() {
        let result = probe("workspace-write").await;
        assert_eq!(result["project_written"], true);
        assert_eq!(result["outside_written"], false);
        assert_eq!(result["protected_written"], false);
        assert_eq!(result["credentials_readable"], false);
        assert_eq!(result["provider_present"], false);
        assert_eq!(result["catalog_present"], false);
        assert_eq!(result["secret_present"], false);
        assert_eq!(result["proxy_present"], false);
        assert_eq!(result["good"], "configured");
    }

    #[tokio::test]
    async fn read_only_policy_does_not_grant_project_writes_to_language_servers() {
        let result = probe("read-only").await;
        assert_eq!(result["project_written"], false);
        assert_eq!(result["outside_written"], false);
        assert_eq!(result["protected_written"], false);
    }
}
