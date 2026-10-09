//! Shared effective definition selection and fresh checkout/handler authorization.
use cyber_core::config::{HookKind, Resolved};
use cyber_core::hooks::{HookCatalog, HookDefinition, HookEvent};
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
