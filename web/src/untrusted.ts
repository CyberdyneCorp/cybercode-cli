import type { SignalKind, UntrustedRecord } from "../../sdk/typescript/src/generated/types.js";

// Injection signal notices (`untrusted-content` → Injection signal notices), worded as
// the TUI words them (cyber-core `untrusted::notice`). Callers render the line and the
// excerpts with `textContent` only: both are text the page or message chose.

/** `permissions.untrusted_content` settings a notice's consequence depends on. */
export interface NoticePolicy { taint: "off" | "signal" | "any"; bypass: "allow" | "deny" }
/** The defaults, until the server reports the effective settings. */
export const DEFAULT_POLICY: NoticePolicy = { taint: "signal", bypass: "allow" };

const KIND_ORDER: SignalKind[] = ["override_instructions", "role_markup", "secret_access", "exfil_url", "command_request", "permission_change", "hidden_text"];

/** What taint does to risky actions in `mode` for an attached client (`permissions-modes` → Taint-aware approvals). */
export function consequence(mode: string, policy: NoticePolicy = DEFAULT_POLICY): string | undefined {
  if (policy.taint === "off") return undefined;
  if (mode === "bypass") return policy.bypass === "deny" ? "Risky actions in this run will be denied (bypass)." : "Taint is off in bypass.";
  if (mode === "dont-ask") return "Risky actions in this run will be denied.";
  return "Risky actions in this run will ask.";
}

export const hasSignals = (record: UntrustedRecord | null | undefined): record is UntrustedRecord =>
  !!record && ((record.signal_count ?? 0) > 0 || (record.signals?.length ?? 0) > 0);

/** The warning line for a record with signals; undefined without. */
export function noticeLine(record: UntrustedRecord | null | undefined, mode: string, policy: NoticePolicy = DEFAULT_POLICY): string | undefined {
  if (!hasSignals(record)) return undefined;
  const kinds = KIND_ORDER.filter((kind) => record.signals?.some((signal) => signal.kind === kind));
  const noun = record.source === "session" || record.source === "channel" ? "message" : "result";
  const line = `! Possible prompt injection in ${record.source} ${noun} (${kinds.join(", ")}).`;
  const then = consequence(mode, policy);
  return then ? `${line} ${then}` : line;
}
