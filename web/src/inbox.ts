import type { InboxRow, MessageRecord } from "../../sdk/typescript/src/generated/types.js";
import { ApiFault, operationKey, type Port } from "./transport.js";

export interface InboxContext { session: string; directory: string; actionable: boolean; signal: AbortSignal }
interface Page { data: MessageRecord[]; cursor?: { next?: string | null } }
export interface HeldMessage { row: InboxRow; message: MessageRecord; identity: string }
interface Decision { message: string; identity: string; action: "release" | "drop"; key: string; status: "submitting" | "uncertain" }
const encode = encodeURIComponent;
const identity = (row: InboxRow, message: MessageRecord) => JSON.stringify([row.message_id, row.digest, row.parts, row.source, row.admitted_seq, message.from, message.to, message.session_id, message.origin_kind, message.collaboration, message.sha256]);

/** Receiver consent stays separate from permission approvals and remote task reports. */
export class MessageInbox {
  rows: InboxRow[] = [];
  messages = new Map<string, MessageRecord>();
  reviewed = new Map<string, string>();
  cursor: string | undefined;
  notice = "";
  loading = false;
  decision: Decision | undefined;
  #generation = 0;
  #reading = false;
  #readError = false;
  #head: string | undefined;
  constructor(readonly api: Port, readonly context: () => InboxContext | undefined, readonly changed: () => void) {}

  reset(): void {
    this.#generation++; this.#reading = false; this.#readError = false; this.rows = []; this.messages.clear(); this.reviewed.clear();
    this.cursor = undefined; this.#head = undefined; this.notice = ""; this.loading = false; this.decision = undefined;
  }
  get held(): HeldMessage[] {
    const session = this.context()?.session;
    return this.rows.flatMap((row) => {
      const message = this.messages.get(row.message_id);
      if (row.status !== "held" || !message || message.direction !== "in" || message.session_id !== session || message.outcome !== "held") return [];
      if (message.origin_kind === "collaboration" && !message.collaboration) return [];
      return [{ row, message, identity: identity(row, message) }];
    });
  }
  get pendingMetadata(): number { return this.rows.filter((row) => row.status === "held").length - this.held.length; }
  review(item: HeldMessage, accepted: boolean): void {
    if (!this.current(item.row.message_id, item.identity)) return;
    if (accepted) this.reviewed.set(item.row.message_id, item.identity); else this.reviewed.delete(item.row.message_id);
    this.changed();
  }
  private current(id: string, expected: string): HeldMessage | undefined { return this.held.find((item) => item.row.message_id === id && item.identity === expected); }
  private matches(context: InboxContext, generation: number): boolean {
    const current = this.context();
    return generation === this.#generation && current?.session === context.session && current.directory === context.directory;
  }
  async refresh(older = false): Promise<void> {
    const context = this.context();
    if (!context || this.#reading || (older && !this.cursor)) return;
    const generation = this.#generation; const cursor = older ? this.cursor : undefined;
    const query = new URLSearchParams({ direction: "in", limit: "200" }); if (cursor) query.set("cursor", cursor);
    this.#reading = true; this.loading = true; this.changed();
    try {
      const options = { directory: context.directory, signal: context.signal };
      const [inbox, history] = await Promise.all([
        this.api.request<{ data: InboxRow[] }>(`/sessions/${encode(context.session)}/inbox`, options),
        this.api.request<Page>(`/sessions/${encode(context.session)}/messaging?${query}`, options),
      ]);
      if (!this.matches(context, generation)) return;
      this.merge(inbox.data, history, older);
    } catch (error) {
      if (!this.matches(context, generation)) return;
      // Do not leave stale sender/content cards actionable after any failed read.
      this.rows = []; this.messages.clear(); this.reviewed.clear();
      this.#readError = true;
      this.notice = error instanceof ApiFault ? error.message : "The message inbox could not be refreshed.";
    } finally {
      if (this.matches(context, generation)) { this.#reading = false; this.loading = false; this.changed(); }
    }
  }
  private merge(rows: InboxRow[], history: Page, older: boolean): void {
    if (this.#readError) { this.notice = ""; this.#readError = false; }
    this.rows = rows;
    // Keep loaded older provenance only while the corresponding input is held.
    const held = new Set(rows.filter((row) => row.status === "held").map((row) => row.message_id));
    for (const id of this.messages.keys()) if (!held.has(id)) this.messages.delete(id);
    for (const message of history.data) if (held.has(message.message_id)) this.messages.set(message.message_id, message);
    this.advance(history, older);
    this.reconcile();
  }
  private advance(history: Page, older: boolean): void {
    if (older) { this.cursor = history.cursor?.next ?? undefined; return; }
    const head = JSON.stringify(history.data.map((message) => message.message_id));
    if (head !== this.#head) { this.#head = head; this.cursor = history.cursor?.next ?? undefined; }
  }
  private reconcile(): void {
    for (const [id, expected] of this.reviewed) if (!this.current(id, expected)) this.reviewed.delete(id);
    if (this.decision && !this.current(this.decision.message, this.decision.identity)) {
      this.decision = undefined; this.notice = "The message changed or was already handled. Review the current inbox.";
    }
  }
  async decide(item: HeldMessage, action: "release" | "drop"): Promise<void> {
    if (!this.context()?.actionable || this.loading || this.decision || !this.current(item.row.message_id, item.identity)) return;
    if (action === "release" && this.reviewed.get(item.row.message_id) !== item.identity) return;
    this.decision = { message: item.row.message_id, identity: item.identity, action, key: operationKey(), status: "submitting" };
    await this.submit();
  }
  async retry(): Promise<void> { if (this.decision?.status === "uncertain") await this.submit(); }
  private async submit(): Promise<void> {
    const context = this.context(); const decision = this.decision;
    if (!context?.actionable || !decision || !this.current(decision.message, decision.identity)) return;
    const generation = this.#generation;
    decision.status = "submitting"; this.changed();
    try {
      await this.api.request(`/sessions/${encode(context.session)}/inbox/${encode(decision.message)}/${decision.action}`, {
        directory: context.directory, signal: context.signal, method: "POST", key: decision.key,
        ...(decision.action === "release" && { body: { delivery: "queue" } }),
      });
      if (!this.matches(context, generation)) return;
      this.decision = undefined; this.notice = decision.action === "release" ? "Release acknowledged. The server rechecks current sender authorization and local policy." : "Message dropped. It will not be admitted.";
      await this.refresh();
    } catch (error) {
      if (!this.matches(context, generation)) return;
      if (!(error instanceof ApiFault) || error.kind === "network") decision.status = "uncertain";
      else { this.decision = undefined; this.reviewed.delete(decision.message); await this.refresh(); }
      this.notice = error instanceof ApiFault ? error.message : "Message decision outcome is unknown. Reconcile before retrying.";
    } finally { if (this.matches(context, generation)) this.changed(); }
  }
}
