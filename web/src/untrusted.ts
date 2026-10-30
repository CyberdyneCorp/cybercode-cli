import type { NoticePolicy, SignalKind, UntrustedRecord } from "../../sdk/typescript/src/generated/types.js";

// Injection signal notices (`untrusted-content` → Injection signal notices), worded as
// the TUI words them (cyber-core `untrusted::notice`); both are checked against
// `eval/fixtures/untrusted-content/notices.json`. Callers render the line and the
// excerpts with `textContent` only: both are text the page or message chose.

export type { NoticePolicy };
/** The defaults, for a server that reports no settings on the Session. */
export const DEFAULT_POLICY: NoticePolicy = { taint: "signal", bypass: "allow" };

const KIND_ORDER: SignalKind[] = ["override_instructions", "role_markup", "secret_access", "exfil_url", "command_request", "permission_change", "hidden_text"];

/** What taint does to risky actions in `mode` for an attached client (`permissions-modes` → Taint-aware approvals). */
export function consequence(mode: string, policy: NoticePolicy = DEFAULT_POLICY): string | undefined {
  if ((policy.taint ?? "signal") === "off") return undefined;
  if (mode === "bypass") return (policy.bypass ?? "allow") === "deny" ? "Risky actions in this run will be denied (bypass)." : "Taint is off in bypass.";
  if (mode === "dont-ask") return "Risky actions in this run will be denied.";
  return "Risky actions in this run will ask.";
}

export const hasSignals = (record: UntrustedRecord | null | undefined): record is UntrustedRecord =>
  !!record && ((record.signal_count ?? 0) > 0 || (record.signals?.length ?? 0) > 0);

// Hidden characters a record's source may hold, shown as `[U+XXXX]` markers after C0
// controls are dropped, as cyber-core `visible_value` shows them.
const CONTROLS = /[\u0000-\u001f]/gu;
const HIDDEN = /[\u007f­͏ᅟᅠ឴឵᠋-᠏​‌‍⁠-⁥‪-‮⁦-⁯ㅤ︀-️﻿ﾠ￰-￸]|[\u{e0000}-\u{e007f}\u{e0100}-\u{e01ef}]/gu;
const marker = (c: string) => `[U+${(c.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, "0")}]`;
/** A record value as people see it: controls dropped, other hidden characters as markers. */
export const visible = (value: string): string => value.replace(CONTROLS, "").replace(HIDDEN, marker);

/** The warning line for a record with signals; undefined without. */
export function noticeLine(record: UntrustedRecord | null | undefined, mode: string, policy: NoticePolicy = DEFAULT_POLICY): string | undefined {
  if (!hasSignals(record)) return undefined;
  const kinds = KIND_ORDER.filter((kind) => record.signals?.some((signal) => signal.kind === kind));
  const noun = record.source === "session" || record.source === "channel" ? "message" : "result";
  const line = `! Possible prompt injection in ${visible(record.source)} ${noun} (${kinds.join(", ")}).`;
  const then = consequence(mode, policy);
  return then ? `${line} ${then}` : line;
}
