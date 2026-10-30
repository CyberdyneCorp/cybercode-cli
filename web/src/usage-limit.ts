import type { Session } from "../../sdk/typescript/src/generated/types.js";

/** `web-client` → Usage limit wait notice: the pending wait of the attached Session. */
export function usageLimitNotice(session: Session | undefined, clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })): string | undefined {
  const wait = session?.usage_limit;
  if (!wait) return undefined;
  return `Usage limit reached. Resuming at ${clock(wait.resume_at_ms)}; Interrupt cancels the wait.`;
}
