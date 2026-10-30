//! `untrusted` records of tool results (`untrusted-content` → Injection signal notices):
//! noted when the registry frames a result, or when a workspace result scanned under
//! `scan: "all"` has hits, and taken by the runtime into the call's
//! `session.tool.settled.1`.

use std::path::Path;
use std::sync::PoisonError;

use cyber_core::untrusted::{Scan, ScanScope, UntrustedRecord};
use cyber_server::runtime::{Invocation, Provenance};

use super::{BuiltinHost, Ctx};
use crate::tools::text;

/// Workspace tools whose output `scan: "all"` scans. Their output is never framed or
/// rewritten, since edits must match its exact bytes.
const SCANNED_WORKSPACE_TOOLS: &[&str] = &["read", "grep", "bash", "powershell"];

impl BuiltinHost {
    /// Note the record of a framed result: present whether or not it had hits.
    pub(crate) fn note_framed(&self, inv: &Invocation, provenance: &Provenance, scan: Scan) {
        let record = UntrustedRecord::new(provenance.source.as_str(), &provenance.origin, scan);
        self.note_untrusted(inv, record);
    }

    fn note_untrusted(&self, inv: &Invocation, record: UntrustedRecord) {
        self.call_untrusted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((inv.session_id.clone(), inv.call_id.clone()), record);
    }

    /// Take, once, the record noted for a call.
    pub(crate) fn take_untrusted(
        &self,
        session_id: &str,
        call_id: &str,
    ) -> Option<UntrustedRecord> {
        self.call_untrusted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(session_id.to_owned(), call_id.to_owned()))
    }

    /// With `permissions.untrusted_content.scan: "all"`, scan a workspace tool's
    /// model-facing output and note a record when it has hits. The output is unchanged.
    pub(super) fn scan_workspace_output(&self, ctx: &Ctx<'_>, output: &str) {
        if !SCANNED_WORKSPACE_TOOLS.contains(&ctx.inv.name.as_str())
            || self.scan_scope(&ctx.location) != ScanScope::All
        {
            return;
        }
        let scan = cyber_core::untrusted::scan(output);
        if scan.is_empty() {
            return;
        }
        let record = UntrustedRecord::new(&ctx.inv.name, &workspace_origin(ctx), scan);
        self.note_untrusted(ctx.inv, record);
    }

    /// The Location's scan scope. A config that cannot be loaded scans everything: the
    /// scan never changes output, so erring toward it is the safe side.
    fn scan_scope(&self, location: &Path) -> ScanScope {
        (self.opts.config)(location).map_or(ScanScope::All, |(config, _)| {
            ScanScope::from_config(&config)
        })
    }
}

/// Where a scanned workspace result came from: the path read or searched, or the
/// command run, with credentials redacted.
fn workspace_origin(ctx: &Ctx<'_>) -> String {
    let input = &ctx.inv.input;
    match ctx.inv.name.as_str() {
        "bash" | "powershell" => cyber_core::memory::redact_credentials(text(input, "command")),
        _ => ctx.resolve(text(input, "path")).display().to_string(),
    }
}
