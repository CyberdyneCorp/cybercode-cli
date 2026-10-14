//! Shared effective definition selection and fresh checkout/handler authorization.
use cyber_core::config::{HookKind, Resolved};
use cyber_core::hooks::{HookCatalog, HookDefinition, HookEvent, HookScope};
use cyber_core::trust::{HookInvocationTrust, TrustStore};

pub(crate) fn authorize(
    resolved: &Resolved,
    trust: &TrustStore,
    invocation_trust: Option<&HookInvocationTrust>,
    pointer: &str,
    event: &HookEvent,
    kind: HookKind,
) -> Result<(HookDefinition, bool), String> {
    let location = &event.identity().location.directory;
    let checkout = std::fs::canonicalize(cyber_core::config::project_root(location))
        .map_err(|error| error.to_string())?;
    if checkout != resolved.trust.checkout_root {
        return Err("hook event Location does not match resolved checkout".into());
    }
    let catalog = HookCatalog::from_config(resolved)?;
    let definition = catalog
        .definitions
        .into_iter()
        .find(|definition| definition.pointer == pointer)
        .ok_or("hook definition is absent from loaded configuration")?;
    if definition.event != event.event() || definition.kind() != kind {
        return Err("hook handler type or event does not match".into());
    }
    if definition.scope.requires_handler_trust() {
        let workspace_approved = match &resolved.trust.digest {
            Some(digest) => trust.is_approved(&checkout, digest),
            None => Ok(false),
        }
        .map_err(|error| error.to_string())?;
        if !resolved.trust.trusted || !workspace_approved {
            return Err("hook checkout configuration is untrusted".into());
        }
    }
    if !definition
        .is_trusted(&checkout, trust, invocation_trust)
        .map_err(|error| error.to_string())?
    {
        return Err("hook handler digest is untrusted".into());
    }
    Ok((definition, catalog.settings.sandbox_all))
}

/// A managed handler may depend only on configuration that neither the checkout nor
/// the invocation can change: every leaf under `pointer` must come from global
/// configuration, so a project, profile, `--config` or `CYBER_CONFIG*` layer can never
/// redirect a managed guard to its own server or model. Other scopes are unaffected.
pub(crate) fn managed_dependency(
    resolved: &Resolved,
    definition: &HookDefinition,
    pointer: &str,
    what: &str,
) -> Result<(), String> {
    if definition.scope != HookScope::Managed {
        return Ok(());
    }
    let prefix = format!("{pointer}/");
    let foreign = resolved.sources.iter().any(|(key, source)| {
        (key == pointer || key.starts_with(&prefix)) && !source.starts_with("global:")
    });
    if foreign {
        return Err(format!(
            "managed hook {what} is not defined by global configuration alone"
        ));
    }
    Ok(())
}

/// The `/mcp/<name>` pointer of a server, escaped like every configuration pointer.
pub(crate) fn mcp_pointer(name: &str) -> String {
    format!("/mcp/{}", name.replace('~', "~0").replace('/', "~1"))
}

/// The evaluator a managed prompt handler may use: the reference must be selected and
/// its provider defined by global configuration alone, and it must be the reference the
/// Session's model resolver serves (`model_roles.evaluator`, then the small and default
/// fallbacks).
pub(crate) fn managed_evaluator(
    resolved: &Resolved,
    definition: &HookDefinition,
    served: Option<&str>,
) -> Result<(), String> {
    if definition.scope != HookScope::Managed {
        return Ok(());
    }
    const CHAIN: [&str; 5] = [
        "/model_roles/evaluator",
        "/model_roles/small",
        "/small_model",
        "/model_roles/default",
        "/model",
    ];
    let pointer = CHAIN
        .into_iter()
        .find(|pointer| {
            resolved
                .value
                .pointer(pointer)
                .is_some_and(|v| v.is_string())
        })
        .ok_or("managed hook evaluator model is not configured")?;
    managed_dependency(resolved, definition, pointer, "evaluator model")?;
    let reference = resolved.value.pointer(pointer).and_then(|v| v.as_str());
    if reference != served {
        return Err("managed hook evaluator model does not match the Session resolver".into());
    }
    let provider = reference
        .and_then(|reference| reference.split_once('/'))
        .map(|(provider, _)| provider)
        .ok_or("managed hook evaluator model reference is invalid")?;
    managed_dependency(
        resolved,
        definition,
        &format!("/providers/{}", provider.replace('~', "~0")),
        "evaluator provider",
    )
}
