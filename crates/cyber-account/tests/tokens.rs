#[path = "support/oidc.rs"]
mod support;
use cyber_account::{Error, Secret};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use support::{Fixture, MemoryVault, Signing, now};
#[tokio::test]
async fn verifies_id_signature_nonce_audience_and_authorized_party() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let account = client
        .verify_id_token(&f.signing.sign(&f.id_claims("nonce")), "nonce")
        .await
        .unwrap();
    assert_eq!(account.sub, "alice");
    assert_eq!(account.email.as_deref(), Some("alice@example.test"));
    let edits = vec![
        ("nonce", json!("wrong")),
        ("aud", json!("other-client")),
        ("azp", json!("other-client")),
        ("iss", json!("https://attacker.example")),
        ("sub", json!("")),
        ("exp", json!(now() - 120)),
        ("iat", json!(now() + 120)),
        ("nbf", json!(now() + 120)),
        ("auth_time", json!(now() + 120)),
    ];
    for (field, value) in edits {
        let mut c = f.id_claims("nonce");
        c[field] = value;
        assert!(
            matches!(
                client.verify_id_token(&f.signing.sign(&c), "nonce").await,
                Err(Error::InvalidToken)
            ),
            "{field}"
        );
    }
    let mut c = f.id_claims("nonce");
    c["aud"] = json!(["cyber-cli", "second"]);
    assert!(matches!(
        client.verify_id_token(&f.signing.sign(&c), "nonce").await,
        Err(Error::InvalidToken)
    ));
    c["azp"] = json!("cyber-cli");
    assert!(
        client
            .verify_id_token(&f.signing.sign(&c), "nonce")
            .await
            .is_ok()
    );
    for missing in ["nonce", "iat", "exp", "aud", "iss", "sub"] {
        let mut c = f.id_claims("nonce");
        c.as_object_mut().unwrap().remove(missing);
        assert!(
            matches!(
                client.verify_id_token(&f.signing.sign(&c), "nonce").await,
                Err(Error::InvalidToken)
            ),
            "{missing}"
        );
    }
}

#[tokio::test]
async fn unknown_kid_refetches_discovery_and_keys_once_and_can_rotate() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let replacement = Signing::new("second");
    *f.state.keys.lock().unwrap() = json!({"keys":[replacement.jwk]});
    client
        .verify_id_token(&replacement.sign(&f.id_claims("n")), "n")
        .await
        .unwrap();
    assert_eq!(f.state.key_requests.load(Ordering::SeqCst), 2);
    assert!(matches!(
        client
            .verify_id_token(&f.signing.sign(&f.id_claims("n")), "n")
            .await,
        Err(Error::UnknownSigningKey)
    ));
    assert_eq!(f.state.key_requests.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn wrong_signatures_hmac_keys_and_duplicate_key_ids_are_rejected() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let attacker = Signing::new("first");
    assert!(matches!(
        client
            .verify_id_token(&attacker.sign(&f.id_claims("n")), "n")
            .await,
        Err(Error::InvalidToken)
    ));
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("first".into());
    let token = encode(
        &header,
        &f.id_claims("n"),
        &EncodingKey::from_secret(b"not-a-public-key"),
    )
    .unwrap();
    assert!(matches!(
        client.verify_id_token(&token, "n").await,
        Err(Error::InvalidToken)
    ));
    *f.state.keys.lock().unwrap() = json!({"keys":[f.signing.jwk,f.signing.jwk]});
    let mut other = f.client().await;
    assert!(matches!(
        other
            .verify_id_token(&f.signing.sign(&f.id_claims("n")), "n")
            .await,
        Err(Error::InvalidToken)
    ));
}

#[tokio::test]
async fn key_use_operations_and_algorithm_are_enforced() {
    let f = Fixture::start().await;
    for (field, value) in [
        ("use", json!("enc")),
        ("key_ops", json!(["sign"])),
        ("alg", json!("RS256")),
    ] {
        let mut key = f.signing.jwk.clone();
        key[field] = value;
        *f.state.keys.lock().unwrap() = json!({"keys":[key]});
        let mut client = f.client().await;
        assert!(matches!(
            client
                .verify_id_token(&f.signing.sign(&f.id_claims("n")), "n")
                .await,
            Err(Error::InvalidToken)
        ));
    }
}

#[tokio::test]
async fn live_introspection_replaces_stale_orgs_and_bounds_authority() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let before = Instant::now();
    let lease = client
        .authorize_human(&f.access(), &f.service(), "hub")
        .await
        .unwrap();
    assert!(lease.is_current());
    assert!(lease.deadline() >= before + Duration::from_secs(59));
    assert!(lease.deadline() <= Instant::now() + Duration::from_secs(60));
    assert_eq!(lease.account().orgs[0].id, "org-uuid");
    assert!(lease.require_organization("org-uuid").is_ok());
    assert_eq!(
        lease.require_organization("stale-jwt-org"),
        Err(Error::InactiveIdentity)
    );
    assert!(lease.require_step_up(true).is_ok());
    assert_eq!(lease.account().roles, ["cyber-cli:reader"]);
    assert_eq!(f.state.checks.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn inactive_service_missing_scope_and_mismatched_live_principals_fail_closed() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let original = f.state.live.lock().unwrap().clone();
    for (field, value) in [
        ("active", json!(false)),
        ("token_type", json!("service")),
        ("sub", json!("mallory")),
        ("aud", json!("other-hub")),
        ("exp", json!(now())),
        ("scope", json!("openid")),
    ] {
        let mut live = original.clone();
        live[field] = value;
        *f.state.live.lock().unwrap() = live;
        assert!(
            matches!(
                client
                    .authorize_human(&f.access(), &f.service(), "hub")
                    .await,
                Err(Error::InactiveIdentity)
            ),
            "{field}"
        );
    }
    *f.state.live.lock().unwrap() = json!({"active":false});
    assert!(matches!(
        client
            .authorize_human(&f.access(), &f.service(), "hub")
            .await,
        Err(Error::InactiveIdentity)
    ));
}

#[tokio::test]
async fn sensitive_revalidation_observes_org_removal_and_disabled_users_immediately() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    client
        .authorize_human(&f.access(), &f.service(), "hub")
        .await
        .unwrap();
    f.state
        .live
        .lock()
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("orgs");
    let lease = client
        .authorize_human(&f.access(), &f.service(), "hub")
        .await
        .unwrap();
    assert_eq!(
        lease.require_organization("org-uuid"),
        Err(Error::InactiveIdentity)
    );
    f.state.live.lock().unwrap()["active"] = json!(false);
    assert!(matches!(
        client
            .authorize_human(&f.access(), &f.service(), "hub")
            .await,
        Err(Error::InactiveIdentity)
    ));
    assert_eq!(f.state.checks.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn signed_scope_and_audience_are_mandatory_before_introspection() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let mut claims = f.claims();
    claims["scope"] = json!("openid");
    assert!(matches!(
        client
            .authorize_human(&Secret::new(f.signing.sign(&claims)), &f.service(), "hub")
            .await,
        Err(Error::MissingCollaborationScope)
    ));
    claims = f.claims();
    claims["aud"] = json!("cyber-cli");
    assert!(matches!(
        client
            .authorize_human(&Secret::new(f.signing.sign(&claims)), &f.service(), "hub")
            .await,
        Err(Error::InvalidToken)
    ));
    assert_eq!(f.state.checks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn live_failure_never_falls_back_to_signed_jwt_or_old_lease() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let lease = client
        .authorize_human(&f.access(), &f.service(), "hub")
        .await
        .unwrap();
    f.task.abort();
    assert!(matches!(
        client
            .authorize_human(&f.access(), &f.service(), "hub")
            .await,
        Err(Error::LiveValidationUnavailable)
    ));
    assert!(lease.deadline() <= Instant::now() + Duration::from_secs(60));
}

#[tokio::test]
async fn step_up_uses_live_authentication_time_and_managed_mfa() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    for (time, amr) in [
        (json!(now() - 901), json!(["pwd", "otp"])),
        (Value::Null, json!(["pwd", "otp"])),
        (json!(now()), json!(["pwd"])),
        (json!(now()), json!(["hwk"])),
    ] {
        f.state.live.lock().unwrap()["auth_time"] = time;
        f.state.live.lock().unwrap()["amr"] = amr;
        let lease = client
            .authorize_human(&f.access(), &f.service(), "hub")
            .await
            .unwrap();
        assert_eq!(lease.require_step_up(true), Err(Error::StepUpRequired));
    }
}

#[tokio::test]
async fn exchange_binds_access_identity_to_verified_id_and_redacts_credentials() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle(&mut client).await;
    assert_eq!(bundle.account().sub, "alice");
    assert_eq!(
        bundle.account().email.as_deref(),
        Some("alice@example.test")
    );
    let debug = format!("{bundle:?}");
    assert!(!debug.contains(bundle.access_token().expose()));
    assert!(!debug.contains("refresh-1"));
    let forms = f.state.forms.lock().unwrap();
    assert_eq!(forms[0]["code"], "auth-code");
    assert!(forms[0].contains_key("code_verifier"));
}

#[tokio::test]
async fn refresh_rotates_credentials_and_cannot_renew_step_up_time() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let old = f.bundle(&mut client).await;
    let mut claims = f.claims();
    claims["auth_time"] = json!(now());
    claims["exp"] = json!(now() + 7200);
    *f.state.exchange.lock().unwrap() = json!({"access_token":f.signing.sign(&claims),"refresh_token":"refresh-2","token_type":"Bearer"});
    let new = client.refresh(&old, "hub").await.unwrap();
    assert_eq!(new.account().auth_time, old.account().auth_time);
    assert!(new.account().expires_at > old.account().expires_at);
    assert_eq!(
        f.state.forms.lock().unwrap()[1]["refresh_token"],
        "refresh-1"
    );
    *f.state.denial.lock().unwrap() = Some("invalid_grant".into());
    assert!(matches!(
        client.refresh(&new, "hub").await,
        Err(Error::SessionExpired)
    ));
}

#[tokio::test]
async fn refresh_rejects_forged_replacement_id_and_subject_switches() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let old = f.bundle(&mut client).await;
    let attacker = Signing::new("first");
    *f.state.exchange.lock().unwrap() = json!({"access_token":f.access().expose(),"id_token":attacker.sign(&f.id_claims("any")),"token_type":"Bearer"});
    assert!(matches!(
        client.refresh(&old, "hub").await,
        Err(Error::InvalidToken)
    ));
    let mut claims = f.claims();
    claims["sub"] = json!("bob");
    *f.state.exchange.lock().unwrap() =
        json!({"access_token":f.signing.sign(&claims),"token_type":"Bearer"});
    assert!(matches!(
        client.refresh(&old, "hub").await,
        Err(Error::InvalidToken)
    ));
}

#[tokio::test]
async fn vault_storage_chunks_credentials_binds_identity_and_keeps_tokens_out_of_index() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle(&mut client).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store =
        cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault.clone()));
    assert_eq!(
        store.save(bundle.clone()).await.unwrap(),
        cyber_account::StorageKind::Keyring
    );
    assert!(!directory.path().join("account.json").exists());
    let index = std::fs::read_to_string(directory.path().join("account-index.json")).unwrap();
    assert!(!index.contains(bundle.access_token().expose()));
    assert!(!index.contains("refresh-1"));
    assert_eq!(
        store.load().await.unwrap().unwrap().access_token().expose(),
        bundle.access_token().expose()
    );
    assert!(vault.values.lock().unwrap().len() > 2);
    let again =
        cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault.clone()));
    assert_eq!(again.load().await.unwrap().unwrap().account().sub, "alice");
    store.logout(true).await.unwrap();
    assert!(store.load().await.unwrap().is_none());
    assert!(vault.values.lock().unwrap().is_empty());
}

#[tokio::test]
async fn vault_chunk_tampering_and_failed_writes_are_not_silently_downgraded() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle(&mut client).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store =
        cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault.clone()));
    store.save(bundle.clone()).await.unwrap();
    let chunk = vault
        .values
        .lock()
        .unwrap()
        .keys()
        .find(|k| k.ends_with(".0"))
        .unwrap()
        .clone();
    vault.values.lock().unwrap().insert(chunk, "corrupt".into());
    assert!(matches!(store.load().await, Err(Error::Storage(_))));
    vault.fail_writes.store(true, Ordering::SeqCst);
    assert!(matches!(store.save(bundle).await, Err(Error::Storage(_))));
    assert!(!directory.path().join("account.json").exists());
}

#[tokio::test]
async fn concurrent_refreshes_reread_rotated_credentials_under_one_lock() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle_until(&mut client, now() + 30).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store = cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault));
    store.save(bundle).await.unwrap();
    *f.state.exchange.lock().unwrap() =
        json!({"access_token":f.access().expose(),"refresh_token":"rotated","token_type":"Bearer"});
    let (a, b) = (f.client().await, f.client().await);
    let (a, b) = tokio::join!(
        store.refresh(a, "hub".into()),
        store.refresh(b, "hub".into())
    );
    assert_eq!(a.unwrap().account().sub, "alice");
    assert_eq!(b.unwrap().account().sub, "alice");
    let requests = f.state.forms.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|f| f["grant_type"] == "refresh_token")
            .count(),
        1
    );
}

#[tokio::test]
async fn invalid_grant_clears_credentials_and_a_durable_pending_marker_blocks_replay() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle_until(&mut client, now() + 30).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store =
        cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault.clone()));
    store.save(bundle.clone()).await.unwrap();
    *f.state.denial.lock().unwrap() = Some("invalid_grant".into());
    assert!(matches!(
        store.refresh(f.client().await, "hub".into()).await,
        Err(Error::SessionExpired)
    ));
    assert!(store.load().await.unwrap().is_none());
    store.save(bundle).await.unwrap();
    let anchor = vault
        .values
        .lock()
        .unwrap()
        .keys()
        .find(|k| {
            !k.ends_with(".0")
                && !k
                    .rsplit('.')
                    .next()
                    .unwrap()
                    .bytes()
                    .all(|b| b.is_ascii_digit())
        })
        .unwrap()
        .clone();
    {
        let mut values = vault.values.lock().unwrap();
        let mut manifest: Value = serde_json::from_str(&values[&anchor]).unwrap();
        manifest["pending"] = json!(true);
        values.insert(anchor, manifest.to_string());
    }
    let before = f.state.forms.lock().unwrap().len();
    assert!(matches!(
        store.refresh(f.client().await, "hub".into()).await,
        Err(Error::SessionExpired)
    ));
    assert_eq!(f.state.forms.lock().unwrap().len(), before);
}

#[cfg(unix)]
#[tokio::test]
async fn private_file_fallback_refuses_public_files_and_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle(&mut client).await;
    let directory = tempfile::tempdir().unwrap();
    let store = cyber_account::AccountStore::with_vault(directory.path().into(), None);
    assert_eq!(
        store.save(bundle.clone()).await.unwrap(),
        cyber_account::StorageKind::File
    );
    let path = directory.path().join("account.json");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(store.load().await.unwrap().unwrap().account().sub, "alice");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(store.load().await, Err(Error::Storage(_))));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let target = directory.path().join("original.json");
    std::fs::rename(&path, &target).unwrap();
    symlink(&target, &path).unwrap();
    assert!(matches!(store.load().await, Err(Error::Storage(_))));
}

#[tokio::test]
async fn cancellation_does_not_release_the_refresh_lock_before_persistence() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle_until(&mut client, now() + 30).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store = cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault));
    store.save(bundle).await.unwrap();
    *f.state.exchange.lock().unwrap() =
        json!({"access_token":f.access().expose(),"refresh_token":"rotated","token_type":"Bearer"});
    let gate = Arc::new(tokio::sync::Notify::new());
    *f.state.delay.lock().unwrap() = Some(gate.clone());
    let refreshed = store.clone();
    let first = f.client().await;
    let task = tokio::spawn(async move { refreshed.refresh(first, "hub".into()).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.state.forms.lock().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    gate.notify_one();
    let result = store.refresh(f.client().await, "hub".into()).await.unwrap();
    assert_eq!(result.account().sub, "alice");
    assert_eq!(
        f.state
            .forms
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f["grant_type"] == "refresh_token")
            .count(),
        1
    );
}

#[tokio::test]
async fn advertised_device_login_polls_pending_and_validates_signed_identity() {
    let f = Fixture::start().await;
    f.state.device_enabled.store(true, Ordering::SeqCst);
    f.state
        .device_errors
        .lock()
        .unwrap()
        .push_back("authorization_pending".into());
    let mut id = f.id_claims("");
    id.as_object_mut().unwrap().remove("nonce");
    *f.state.exchange.lock().unwrap() = json!({"access_token":f.access().expose(),"id_token":f.signing.sign(&id),"refresh_token":"device-refresh","token_type":"Bearer"});
    let mut client = f.client().await;
    let device = client.start_device_authorization().await.unwrap();
    assert_eq!(device.user_code(), "ABCD-EFGH");
    assert_eq!(device.verification_uri().path(), "/device-ui");
    let account = client
        .poll_device_authorization(device, "hub")
        .await
        .unwrap();
    assert_eq!(account.account().sub, "alice");
    assert_eq!(f.state.device_polls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn device_login_honors_slow_down_and_reports_denial_without_exposing_code() {
    let f = Fixture::start().await;
    f.state.device_enabled.store(true, Ordering::SeqCst);
    f.state
        .device_errors
        .lock()
        .unwrap()
        .extend(["slow_down".into(), "access_denied".into()]);
    let mut client = f.client().await;
    let device = client.start_device_authorization().await.unwrap();
    let started = Instant::now();
    let error = client
        .poll_device_authorization(device, "hub")
        .await
        .err()
        .unwrap();
    assert_eq!(error, Error::AuthorizationDenied);
    assert!(started.elapsed() >= Duration::from_secs(7));
    assert!(!format!("{error:?}").contains("private-device-code"));
}

#[tokio::test]
async fn device_login_is_unavailable_without_advertisement_and_expires_without_replaying() {
    let f = Fixture::start().await;
    let client = f.client().await;
    assert!(matches!(
        client.start_device_authorization().await,
        Err(Error::PkceUnsupported)
    ));
    f.state.device_enabled.store(true, Ordering::SeqCst);
    f.state.device_expiry.store(2, Ordering::SeqCst);
    f.state
        .device_errors
        .lock()
        .unwrap()
        .extend(std::iter::repeat_n("authorization_pending".into(), 4));
    let mut client = f.client().await;
    let device = client.start_device_authorization().await.unwrap();
    assert!(matches!(
        client.poll_device_authorization(device, "hub").await,
        Err(Error::LoginExpired)
    ));
    assert_eq!(f.state.device_polls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn logout_invalidates_an_older_pending_login_before_any_vault_write() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let bundle = f.bundle(&mut client).await;
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store =
        cyber_account::AccountStore::with_vault(directory.path().into(), Some(vault.clone()));
    let epoch = store.login_epoch().await.unwrap();
    store.logout(true).await.unwrap();
    assert_eq!(
        store.save_login(bundle, epoch).await.unwrap_err(),
        Error::LoginExpired
    );
    assert!(vault.values.lock().unwrap().is_empty());
    assert!(store.load().await.unwrap().is_none());
}

#[tokio::test]
async fn backend_credentials_and_concurrent_live_identities_are_independent() {
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let backend = client
        .client_credentials("backend", &Secret::new("service-secret".into()), None)
        .await
        .unwrap();
    assert!(!backend.needs_refresh());
    assert_eq!(f.state.service_tokens.load(Ordering::SeqCst), 1);
    let alice = f.user("alice", json!([]));
    let bob = f.user("bob", json!([]));
    assert_eq!(
        client
            .authorize_human(&alice, &backend.token, "hub")
            .await
            .unwrap()
            .account()
            .sub,
        "alice"
    );
    assert_eq!(
        client
            .authorize_human(&bob, &backend.token, "hub")
            .await
            .unwrap()
            .account()
            .sub,
        "bob"
    );
    f.state.users.lock().unwrap().get_mut("alice").unwrap()["active"] = json!(false);
    assert!(
        client
            .authorize_human(&alice, &backend.token, "hub")
            .await
            .is_err()
    );
    assert!(
        client
            .authorize_human(&bob, &backend.token, "hub")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn reauthentication_checks_auth_time_even_if_issuer_ignores_max_age() {
    use cyber_account::AuthorizationAttempt;
    let f = Fixture::start().await;
    let mut client = f.client().await;
    for age in [Some(901), None, Some(0)] {
        let attempt = AuthorizationAttempt::new(
            client.provider(),
            "cyber-cli",
            "http://127.0.0.1:12345/callback".parse().unwrap(),
        )
        .unwrap()
        .require_recent_authentication();
        let params: std::collections::HashMap<_, _> = attempt
            .authorization_url()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(params["prompt"], "login");
        assert_eq!(params["max_age"], "900");
        let mut id = f.id_claims(&params["nonce"]);
        match age {
            Some(age) => id["auth_time"] = json!(now() - age),
            None => {
                id.as_object_mut().unwrap().remove("auth_time");
            }
        }
        *f.state.exchange.lock().unwrap() = json!({"access_token":f.access().expose(),"id_token":f.signing.sign(&id),"refresh_token":"refresh","token_type":"Bearer"});
        let grant = attempt
            .finish(&format!(
                "http://127.0.0.1:12345/callback?state={}&code=auth-code",
                params["state"]
            ))
            .unwrap();
        let result = client.exchange(grant, "hub").await;
        if age == Some(0) {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(Error::StepUpRequired)));
        }
    }
}

#[tokio::test]
async fn host_secrets_and_accounts_bind_native_credentials_to_installation() {
    use cyber_account::{AccountStore, HostSecrets, StorageKind};
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let vault = Arc::new(MemoryVault::default());
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let a = AccountStore::with_vault(first.path().into(), Some(vault.clone()));
    let b = AccountStore::with_vault(second.path().into(), Some(vault.clone()));
    a.save(f.bundle(&mut client).await).await.unwrap();
    b.save(f.bundle(&mut client).await).await.unwrap();
    a.logout(true).await.unwrap();
    assert!(b.load().await.unwrap().is_some());
    let secrets = HostSecrets::new(
        first.path().into(),
        "capabilities.json",
        "test",
        Some(vault.clone()),
    )
    .unwrap();
    assert_eq!(
        secrets
            .put("agent", Secret::new("capability-secret".into()))
            .await
            .unwrap(),
        StorageKind::Keyring
    );
    assert!(
        !std::fs::read_to_string(secrets.file_path())
            .unwrap()
            .contains("capability-secret")
    );
    assert_eq!(
        secrets.get("agent").await.unwrap().unwrap().expose(),
        "capability-secret"
    );
    let other = HostSecrets::new(
        second.path().into(),
        "capabilities.json",
        "test",
        Some(vault),
    )
    .unwrap();
    assert!(other.get("agent").await.unwrap().is_none());
    secrets.delete("agent").await.unwrap();
    assert!(secrets.get("agent").await.unwrap().is_none());
}

#[tokio::test]
async fn failed_native_chunk_commit_is_tracked_and_removed_before_next_operation() {
    use cyber_account::{AccountStore, CredentialVault};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    struct PartialVault {
        memory: MemoryVault,
        writes: AtomicUsize,
        fail_once: AtomicBool,
    }
    impl CredentialVault for PartialVault {
        fn get(&self, k: &str) -> Result<Option<Secret>, Error> {
            self.memory.get(k)
        }
        fn put(&self, k: &str, v: &str) -> Result<(), Error> {
            if self.writes.fetch_add(1, Ordering::SeqCst) == 1
                && self.fail_once.swap(false, Ordering::SeqCst)
            {
                return Err(Error::Storage("injected partial write"));
            }
            self.memory.put(k, v)
        }
        fn delete(&self, k: &str) -> Result<(), Error> {
            self.memory.delete(k)
        }
    }
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(PartialVault {
        memory: MemoryVault::default(),
        writes: AtomicUsize::new(0),
        fail_once: AtomicBool::new(true),
    });
    let store = AccountStore::with_vault(dir.path().into(), Some(vault.clone()));
    assert!(store.save(f.bundle(&mut client).await).await.is_err());
    assert!(dir.path().join("account-vault-pending.json").exists());
    assert!(!vault.memory.values.lock().unwrap().is_empty());
    store.logout(true).await.unwrap();
    assert!(vault.memory.values.lock().unwrap().is_empty());
    assert!(!dir.path().join("account-vault-pending.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn host_secret_file_fallback_is_private_and_deletable() {
    use cyber_account::{HostSecrets, StorageKind};
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let secrets = HostSecrets::new(dir.path().into(), "secrets.json", "test", None).unwrap();
    assert_eq!(
        secrets
            .put("agent", Secret::new("capability".into()))
            .await
            .unwrap(),
        StorageKind::File
    );
    assert_eq!(
        std::fs::metadata(secrets.file_path()).unwrap().mode() & 0o077,
        0
    );
    assert_eq!(
        secrets.get("agent").await.unwrap().unwrap().expose(),
        "capability"
    );
    let different_namespace =
        HostSecrets::new(dir.path().into(), "secrets.json", "different", None).unwrap();
    assert!(different_namespace.get("agent").await.is_err());
    secrets.delete("agent").await.unwrap();
    assert!(secrets.get("agent").await.unwrap().is_none());
}

#[tokio::test]
async fn interrupted_native_index_commit_remains_reachable_for_logout() {
    use cyber_account::AccountStore;
    let f = Fixture::start().await;
    let mut client = f.client().await;
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(MemoryVault::default());
    let store = AccountStore::with_vault(dir.path().into(), Some(vault.clone()));
    store.save(f.bundle(&mut client).await).await.unwrap();
    let manifest: Value = vault
        .values
        .lock()
        .unwrap()
        .values()
        .find_map(|v| {
            serde_json::from_str::<Value>(v)
                .ok()
                .filter(|v| v.get("generation").is_some())
        })
        .unwrap();
    std::fs::remove_file(dir.path().join("account-index.json")).unwrap();
    let journal = dir.path().join("account-vault-pending.json");
    std::fs::write(&journal, serde_json::to_vec(&json!({"key":manifest["key"],"generation":manifest["generation"],"chunks":manifest["chunks"]})).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    // Recovery keeps unfinished login inactive while making its committed native
    // credential discoverable to logout and later cleanup.
    assert!(store.load().await.unwrap().is_none());
    store.logout(true).await.unwrap();
    assert!(vault.values.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn uncertain_native_host_secret_write_cannot_revive_old_file_fallback() {
    use cyber_account::HostSecrets;
    let dir = tempfile::tempdir().unwrap();
    let file = HostSecrets::new(dir.path().into(), "secrets.json", "test", None).unwrap();
    file.put("key", Secret::new("old-file-secret".into()))
        .await
        .unwrap();
    let vault = Arc::new(MemoryVault::default());
    vault.fail_writes.store(true, Ordering::SeqCst);
    let native = HostSecrets::new(dir.path().into(), "secrets.json", "test", Some(vault)).unwrap();
    assert!(
        native
            .put("key", Secret::new("replacement".into()))
            .await
            .is_err()
    );
    assert!(native.get("key").await.is_err());
    assert!(file.get("key").await.is_err());
    assert!(
        !std::fs::read_to_string(file.file_path())
            .unwrap()
            .contains("old-file-secret")
    );
}
