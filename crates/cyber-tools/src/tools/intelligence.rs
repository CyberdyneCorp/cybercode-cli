use crate::{host::Ctx, lsp::ReadOrigin};
use cyber_core::config::LspSettings;
use std::{path::Path, sync::atomic::Ordering, time::Duration};

pub(crate) fn capture(ctx: &Ctx<'_>, path: &Path) -> Option<ReadOrigin> {
    ctx.host.lsp.get()?;
    let settings = LspSettings::from_config(&ctx.host.config_for(&ctx.location)).ok()?;
    if !settings.enabled {
        return None;
    }
    crate::lsp::edit_origin(&ctx.location, path).ok()
}

pub(crate) async fn feedback(
    ctx: &Ctx<'_>,
    path: &Path,
    bytes: &[u8],
    origin: Option<ReadOrigin>,
) -> String {
    let Some(origin) = origin else {
        return String::new();
    };
    let Some(locations) = ctx.host.lsp.get() else {
        return String::new();
    };
    let Ok(settings) = LspSettings::from_config(&ctx.host.config_for(&ctx.location)) else {
        return String::new();
    };
    if !settings.enabled || bytes.len() > crate::lsp::MAX_DOCUMENT_BYTES {
        return String::new();
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return String::new();
    };
    let feedback = locations.feedback(
        &ctx.location,
        path.into(),
        text.into(),
        origin,
        Duration::from_millis(settings.diagnostics_wait_ms),
        ctx.inv.name == "write",
    );
    let blocks = tokio::select! { biased; _ = ctx.cancel.cancelled() => String::new(), blocks = feedback => blocks };
    if blocks.is_empty() {
        return blocks;
    }
    ctx.compiler_feedback.store(true, Ordering::Release);
    format!("\n{blocks}")
}
