// `web-client` → Redaction badge on tool results: a settled call that hid secrets from the
// model says how many in one badge; the output itself shows only the markers.

export interface Redacted {
  redactions?: { count: number } | null;
}

export function redactionNote(tool: Redacted): string | undefined {
  const count = tool.redactions?.count ?? 0;
  if (count <= 0) return undefined;
  return `${count} ${count === 1 ? "secret" : "secrets"} hidden from the model`;
}
