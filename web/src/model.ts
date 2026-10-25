import { Integrations } from "./integrations.js";
import type { EventEnvelope, FileDiff, GoalsView, Job, LocationInfo, Message, PendingRequest, RunDetail, Session, SessionRow, WorkflowRunSummary } from "../../sdk/typescript/src/generated/types.js";
import { ApiFault, operationKey, type Port, type RequestOptions } from "./transport.js";

export type Connection = "authentication required" | "connecting" | "connected" | "reconnecting" | "offline";
export type Tab = "conversation" | "approvals" | "runs" | "integrations";
export interface Lease { client_id: string; session_id: string; expires_at: number }
interface Page<T> { data: T[]; cursor: { next?: string | null } }
interface Located<T> { location: LocationInfo; data: T }
export interface Decision { key: string; body: unknown; fingerprint: string; status: "submitting" | "uncertain" }
export interface Card { request: PendingRequest; key: string; fingerprint: string; generation: number; decision?: Decision }
export interface Command { key: string; path: string; body?: unknown; method: "POST" | "DELETE"; description: string; session: string; status: "submitting" | "uncertain" }
export interface State {
  authenticated: boolean; authBusy: boolean; retryAt: number; connection: Connection;
  directory: string; location?: LocationInfo | undefined; sessions: SessionRow[]; sessionCursor?: string | undefined;
  session?: Session | undefined; messages: Message[]; messageCursor?: string | undefined; diffs: FileDiff[];
  cards: Card[]; runs: WorkflowRunSummary[]; details: Map<string, RunDetail>; jobs: Job[];
  goals?: GoalsView | undefined; unavailable: Set<string>; lease?: Lease | undefined; notice: string;
  loading: boolean; olderTranscript: boolean; tab: Tab; draft: string; delivery: "steer" | "queue";
  review?: Card | undefined; reviewed: boolean; feedback?: Card | undefined; answers: Map<string, string[][]>;
  customs: Map<string, string[]>; command?: Command | undefined; liveText: string;
  feedbackDraft: string; creation?: { key: string; directory: string; status: "submitting" | "uncertain" } | undefined;
}
const initial = (): State => ({ authenticated: false, authBusy: false, retryAt: 0, connection: "authentication required", directory: "", sessions: [], messages: [], diffs: [], cards: [], runs: [], details: new Map(), jobs: [], unavailable: new Set(), notice: "", loading: false, olderTranscript: false, tab: "conversation", draft: "", delivery: "steer", reviewed: false, answers: new Map(), customs: new Map(), liveText: "", feedbackDraft: "" });
const data = <T>(value: { data: T }): T => value.data;
export const requestKey = (request: PendingRequest) => `${request.session_id}/${request.id}`;
export function fingerprint(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(fingerprint).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, item]) => `${JSON.stringify(key)}:${fingerprint(item)}`).join(",")}}`;
  return JSON.stringify(value) ?? "null";
}
const encoded = encodeURIComponent;
const pause = (ms: number, signal: AbortSignal) => new Promise<void>((resolve) => {
  if (signal.aborted) return resolve();
  const timer = setTimeout(resolve, ms);
  signal.addEventListener("abort", () => { clearTimeout(timer); resolve(); }, { once: true });
});

/** State, authority and uncertainty live here; the view never calls HTTP directly. */
export class Dashboard {
  state = initial();
  changed: () => void = () => {};
  #generation = 0;
  #failures = 0;
  #identity: string | undefined;
  #attachment = new AbortController();
  #leaseKey: string | undefined;
  #renewal: ReturnType<typeof setTimeout> | undefined;
  #refreshTimer: ReturnType<typeof setTimeout> | undefined;
  #refreshing: Promise<void> | undefined;
  #recovery: Promise<void> | undefined;
  #opened = new Set<string>();
  #cursors = new Map<string, number>();
  #decisions = new Map<string, Decision>();
  #commands = new Map<string, Command>();
  #networkRecovery: Promise<void> | undefined;
  #pageCursor: string | undefined;
  #pageVersion = 0;
  readonly integrations: Integrations;
  constructor(readonly api: Port, readonly now = Date.now) { this.integrations = new Integrations(api, () => ({directory:this.state.directory,session:this.state.session?.id,enabled:this.actionable}), () => this.emit()); }
  emit(): void { this.changed(); }
  get actionable(): boolean { return this.state.connection === "connected" && !!this.state.lease && this.state.lease.expires_at > this.now(); }
  async login(password: string): Promise<void> {
    if (this.state.authBusy || this.now() < this.state.retryAt) return;
    const generation = ++this.#generation;
    this.state.authBusy = true; this.state.notice = ""; this.state.connection = "connecting"; this.emit();
    this.api.password(password);
    try {
      const location = data(await this.api.request<{ data: LocationInfo }>("/location"));
      if (generation !== this.#generation) return;
      this.state.authenticated = true; this.state.location = location; this.state.directory = location.directory;
      await this.checkIdentity();
      await this.loadSessions();
      this.#failures = 0;
      if (this.state.sessions[0]) await this.selectSession(this.state.sessions[0].id);
      else this.state.connection = "connected";
    } catch (error) {
      if (generation === this.#generation) this.authenticationRequired(error instanceof ApiFault && error.kind === "network" ? "Server unavailable. Sign in again when it is reachable." : "Sign-in was not accepted. Check the local server password.");
      this.#failures++;
      this.state.retryAt = this.now() + Math.min(8000, 500 * 2 ** Math.min(4, this.#failures - 1));
    } finally { this.state.authBusy = false; this.emit(); }
  }
  authenticationRequired(message = "Your connection needs authentication. Sign in again."): void {
    this.integrations.reset(); this.stopAttachment(); this.api.clear(); this.#identity = undefined; this.#decisions.clear(); this.#commands.clear(); this.#cursors.clear(); this.jobLog = undefined;
    this.state = initial(); this.state.notice = message; this.emit();
  }
  logout(): void {
    this.releaseLease(); this.authenticationRequired("Signed out. The server and its running work continue.");
  }
  async switchLocation(directory: string): Promise<void> {
    if (!this.state.authenticated || this.state.loading) return;
    this.integrations.reset(); this.releaseLease(); this.stopAttachment();
    const generation = this.#generation; this.jobLog = undefined;
    this.state.session = undefined; this.state.cards = []; this.state.messages = []; this.state.diffs = []; this.state.runs = []; this.state.jobs = []; this.state.goals = undefined; this.state.directory = directory; this.state.loading = true; this.state.connection = "connecting"; this.emit();
    try {
      const location = data(await this.api.request<{ data: LocationInfo }>("/location", { directory }));
      if (generation !== this.#generation) return;
      this.state.location = location; this.state.directory = location.directory;
      await this.loadSessions();
      if (generation !== this.#generation) return;
      if (this.state.sessions[0]) await this.selectSession(this.state.sessions[0].id);
      else this.state.connection = "connected";
    } catch (error) { if (generation === this.#generation) this.showError(error); } finally { if (generation === this.#generation) { this.state.loading = false; this.emit(); } }
  }
  async loadSessions(cursor?: string): Promise<void> {
    const generation = this.#generation;
    const query = cursor ? `?limit=50&cursor=${encoded(cursor)}` : "?limit=50";
    const result = await this.api.request<Located<Page<SessionRow>>>(`/sessions${query}`, { directory: this.state.directory });
    if (generation !== this.#generation) return;
    this.state.sessions = cursor ? [...this.state.sessions, ...result.data.data] : result.data.data;
    this.state.sessionCursor = result.data.cursor.next ?? undefined;
    this.emit();
  }
  async moreSessions(): Promise<void> {
    const generation = this.#generation;
    try { await this.loadSessions(this.state.sessionCursor); } catch (error) { if (generation === this.#generation) this.showError(error); }
  }
  async selectSession(id: string): Promise<void> {
    if (!this.state.authenticated) return;
    this.integrations.reset(); this.releaseLease(); this.stopAttachment();
    const generation = this.#generation;
    this.jobLog = undefined;
    this.state.loading = true; this.state.connection = "connecting"; this.state.session = undefined; this.state.diffs = []; this.state.runs = []; this.state.jobs = []; this.state.goals = undefined; this.state.cards = []; this.state.messages = []; this.state.liveText = ""; this.state.draft = ""; this.state.command = this.#commands.get(id); this.state.review = undefined; this.state.feedback = undefined; this.state.details.clear(); this.emit();
    try {
      const session = data(await this.api.request<{ data: Session }>(`/sessions/${encoded(id)}`, this.options()));
      if (generation !== this.#generation) return;
      this.state.session = session;
      await this.refresh();
      await this.attachLease(generation);
      if (generation !== this.#generation) return;
      this.startStreams(generation); this.scheduleRenewal(generation);
    } catch (error) {
      if (generation === this.#generation) {
        this.showError(error);
        if (this.state.session && this.state.authenticated) { this.startStreams(generation); this.ensureRecovery(generation); }
      }
    }
    finally { if (generation === this.#generation) { this.state.loading = false; this.emit(); } }
  }
  private ensureRecovery(generation: number): void {
    if (this.#networkRecovery) return;
    const work = this.initialRecovery(generation); this.#networkRecovery = work;
    void work.finally(() => { if (this.#networkRecovery === work) this.#networkRecovery = undefined; });
  }
  private async initialRecovery(generation: number): Promise<void> {
    const signal = this.#attachment.signal; let backoff = 500;
    while (!signal.aborted && generation === this.#generation) {
      await pause(backoff, signal);
      if (signal.aborted || generation !== this.#generation) return;
      try { await this.recover(generation); this.scheduleRenewal(generation); return; }
      catch (error) { this.showError(error); backoff = Math.min(15_000, backoff * 2); }
    }
  }
  setTab(tab: Tab): void { this.state.tab = tab; if (tab === "integrations") void this.integrations.load(); this.emit(); }
  setDraft(value: string): void { this.state.draft = value; }
  async olderMessages(): Promise<void> {
    if (this.state.messageCursor) await this.transcriptPage(this.state.messageCursor);
  }
  async latestMessages(): Promise<void> { await this.transcriptPage(); }
  private transcriptPath(session: string, cursor?: string): string {
    return `/sessions/${encoded(session)}/messages?limit=50&order=desc${cursor ? `&cursor=${encoded(cursor)}` : ""}`;
  }
  private async transcriptPage(cursor?: string): Promise<void> {
    const session = this.state.session; const generation = this.#generation;
    if (!session || !this.state.authenticated) return;
    this.#pageCursor = cursor; const version = ++this.#pageVersion;
    this.state.messages = []; this.state.messageCursor = undefined; this.state.olderTranscript = !!cursor; this.emit();
    try {
      const page = await this.api.request<Page<Message>>(this.transcriptPath(session.id, cursor), this.options());
      this.applyTranscript(page, generation, version, cursor);
    } catch (error) { if (generation === this.#generation && version === this.#pageVersion) this.showError(error); }
  }
  private applyTranscript(page: Page<Message>, generation: number, version: number, cursor?: string): void {
    if (generation !== this.#generation || version !== this.#pageVersion) return;
    if (cursor && !page.data.length) { void this.latestMessages(); return; }
    this.state.messages = page.data.reverse(); this.state.messageCursor = page.cursor.next ?? undefined;
    this.state.olderTranscript = !!cursor; this.emit();
  }
  refresh(): Promise<void> {
    if (this.#refreshing) return this.#refreshing;
    const work = this.readSnapshot(); this.#refreshing = work;
    return work.finally(() => { if (this.#refreshing === work) this.#refreshing = undefined; });
  }
  private async readSnapshot(): Promise<void> {
    const session = this.state.session; const generation = this.#generation;
    if (!session || !this.state.authenticated) return;
    const base = `/sessions/${encoded(session.id)}`; const options = this.options();
    const pageCursor = this.#pageCursor; const pageVersion = this.#pageVersion;
    const [current, messages, permissions, questions, diffs, runs, jobs, goals] = await Promise.all([
      this.api.request<{ data: Session }>(base, options),
      this.api.request<Page<Message>>(this.transcriptPath(session.id, pageCursor), options),
      this.api.request<{ data: PendingRequest[] }>(`/permissions/requests?session_id=${encoded(session.id)}`, options),
      this.api.request<{ data: PendingRequest[] }>(`/questions/requests?session_id=${encoded(session.id)}`, options),
      this.optional<{ data: FileDiff[] }>(`${base}/diff`, "Diffs", options),
      this.optional<Page<WorkflowRunSummary>>(`/workflows/runs?session_id=${encoded(session.id)}&limit=50`, "Workflows", options),
      this.optional<Page<Job>>(`/jobs?session_id=${encoded(session.id)}&limit=50`, "Jobs", options),
      this.optional<{ data: GoalsView }>(`${base}/goals`, "Goals", options),
    ]);
    if (generation !== this.#generation) return;
    if (current.data.directory !== session.directory) { void this.selectSession(session.id); return; }
    this.state.session = current.data;
    this.applyTranscript(messages, generation, pageVersion, pageCursor);
    this.state.diffs = diffs?.data ?? []; this.state.runs = runs?.data ?? []; this.state.jobs = jobs?.data ?? []; this.state.goals = goals?.data;
    this.reconcileCards([...permissions.data, ...questions.data], generation);
    if (current.data.status !== "running") this.state.liveText = "";
    this.emit();
  }
  private async optional<T>(path: string, label: string, options: RequestOptions): Promise<T | undefined> {
    const generation = this.#generation;
    try { const result = await this.api.request<T>(path, options); if (generation === this.#generation) this.state.unavailable.delete(label); return result; }
    catch (error) { if (error instanceof ApiFault && (error.kind === "missing" || error.status === 503)) { if (generation === this.#generation) this.state.unavailable.add(label); return undefined; } throw error; }
  }
  private reconcileCards(requests: PendingRequest[], generation: number): void {
    const previous = new Map(this.state.cards.map((card) => [card.key, card]));
    this.state.cards = requests.map((request) => {
      const key = requestKey(request); const snapshot = fingerprint(request);
      const old = previous.get(key); const decision = this.#decisions.get(key);
      if (old?.fingerprint === snapshot) return old;
      if (decision && decision.fingerprint !== snapshot) this.#decisions.delete(key);
      return { request: structuredClone(request), key, fingerprint: snapshot, generation, ...(decision?.fingerprint === snapshot && { decision }) };
    });
    const current = new Set(this.state.cards.map((card) => card.key));
    for (const card of previous.values()) if (!current.has(card.key)) {
      this.#decisions.delete(card.key);
      this.state.notice = "Already answered. The current request list was reconciled with the server.";
    }
    if (this.state.review && !this.currentCard(this.state.review)) this.state.review = undefined;
    if (this.state.feedback && !this.currentCard(this.state.feedback)) this.state.feedback = undefined;
  }
  private currentCard(card: Card): boolean { return card.generation === this.#generation && this.state.cards.some((current) => current === card && current.fingerprint === card.fingerprint); }
  review(card: Card): void { if (this.currentCard(card)) { this.state.review = card; this.state.reviewed = false; this.emit(); } }
  feedback(card: Card): void { if (this.currentCard(card)) { this.state.feedback = card; this.state.feedbackDraft = ""; this.emit(); } }
  async answer(card: Card, body: unknown, retry = false): Promise<void> {
    if (!this.actionable || !this.currentCard(card) || card.decision?.status === "submitting") return;
    if (card.decision && !retry) return;
    if (!retry && !this.validAnswer(card, body)) return;
    const decision = card.decision ?? { key: operationKey(), body: structuredClone(body), fingerprint: card.fingerprint, status: "submitting" as const };
    if (decision.fingerprint !== card.fingerprint) return;
    card.decision = decision; decision.status = "submitting"; this.#decisions.set(card.key, decision); this.state.review = undefined; this.state.feedback = undefined; this.emit();
    const generation = this.#generation;
    try {
      // Reconcile immediately before answering the immutable owner/request/payload.
      const kind = card.request.kind === "permission" ? "permissions" : "questions";
      const current = data(await this.api.request<{ data: PendingRequest[] }>(`/${kind}/requests?session_id=${encoded(this.state.session!.id)}`, this.options()));
      const exact = current.find((request) => requestKey(request) === card.key);
      if (!exact || fingerprint(exact) !== decision.fingerprint) { await this.refresh(); return; }
      if (!this.actionable || !this.currentCard(card)) return;
      await this.api.request(`/sessions/${encoded(card.request.session_id)}/${kind}/${encoded(card.request.id)}/reply`, { ...this.options(), method: "POST", body: decision.body, key: decision.key });
      if (generation !== this.#generation) return;
      this.#decisions.delete(card.key); this.state.cards = this.state.cards.filter((current) => current !== card); this.state.notice = "Answer acknowledged by the server.";
      await this.refresh();
    } catch (error) {
      const uncertain = !(error instanceof ApiFault) || error.kind === "network";
      if (uncertain) decision.status = "uncertain";
      else { this.#decisions.delete(card.key); delete card.decision; }
      if (generation !== this.#generation) return;
      if (error instanceof ApiFault && (error.kind === "missing" || error.kind === "changed")) { await this.refresh().catch(() => {}); this.state.notice = "Already answered or changed. Review the current requests."; }
      else this.showError(error);
    } finally { if (generation === this.#generation) this.emit(); }
  }
  private validAnswer(card: Card, body: unknown): boolean {
    if (!body || typeof body !== "object") return false;
    const value = body as Record<string, unknown>;
    if (card.request.kind === "permission") {
      if (value.reply === "always") return this.state.review === card && this.state.reviewed && card.request.always_patterns.length > 0;
      return value.reply === "once" || (value.reply === "reject" && (value.message === undefined || typeof value.message === "string"));
    }
    if (value.answers === undefined || value.answers === null) return true;
    if (!Array.isArray(value.answers) || value.answers.length !== card.request.questions.length) return false;
    return value.answers.every((answers, index) => {
      const question = card.request.kind === "question" ? card.request.questions[index] : undefined;
      return !!question && Array.isArray(answers) && answers.length > 0 && (question.multi_select || answers.length === 1) && answers.every((answer) => typeof answer === "string" && (question.allow_custom !== false || question.options.some((option) => option.label === answer)));
    });
  }
  async createSession(): Promise<void> {
    if (!this.state.authenticated || this.state.connection !== "connected" || this.state.creation?.status === "submitting") return;
    const generation = this.#generation;
    const creation = this.state.creation ?? { key: operationKey(), directory: this.state.directory, status: "submitting" as const };
    creation.status = "submitting"; this.state.creation = creation; this.emit();
    try {
      const session = data(await this.api.request<{ data: Session }>("/sessions", { method: "POST", body: {}, key: creation.key, directory: creation.directory }));
      if (generation !== this.#generation) return;
      this.state.creation = undefined; await this.loadSessions(); await this.selectSession(session.id);
    } catch (error) {
      if (generation !== this.#generation) return;
      if (!(error instanceof ApiFault) || error.kind === "network") creation.status = "uncertain"; else this.state.creation = undefined;
      this.showError(error);
    } finally { if (generation === this.#generation) this.emit(); }
  }
  async prompt(): Promise<void> {
    const session = this.state.session; const text = this.state.draft.trim();
    if (!session || !text || !this.actionable || this.state.command) return;
    await this.command({ path: `/sessions/${encoded(session.id)}/prompt`, body: { parts: [{ type: "text", text }], delivery: this.state.delivery }, description: "Message", method: "POST" });
  }
  async interrupt(): Promise<void> {
    const session = this.state.session; if (!session || !this.actionable) return;
    await this.command({ path: `/sessions/${encoded(session.id)}/interrupt`, method: "POST", description: "Interrupt" });
  }
  async control(path: string, description: string, method: "POST" | "DELETE" = "POST"): Promise<void> {
    if (!this.actionable) return;
    await this.command({ path, method, description });
  }
  async retryCommand(): Promise<void> { if (this.state.command?.status === "uncertain" && this.actionable) await this.command(this.state.command); }
  private async command(input: Pick<Command, "path" | "description" | "method"> & { body?: unknown; key?: string }): Promise<void> {
    const session = this.state.session; const generation = this.#generation;
    if (!session || !this.actionable || (this.state.command && (this.state.command.status === "submitting" || input.key !== this.state.command.key))) return;
    const command: Command = { ...input, key: input.key ?? operationKey(), session: session.id, status: "submitting" };
    this.state.command = command; this.#commands.set(session.id, command); this.emit();
    try {
      await this.api.request(command.path, { ...this.options(), method: command.method, ...(command.body !== undefined && { body: command.body }), key: command.key });
      this.#commands.delete(command.session);
      if (generation !== this.#generation) return;
      this.state.command = undefined; if (command.description === "Message") this.state.draft = "";
      this.state.notice = `${command.description} acknowledged by the server.`; await this.refresh();
    } catch (error) {
      const uncertain = !(error instanceof ApiFault) || error.kind === "network";
      if (uncertain) command.status = "uncertain"; else this.#commands.delete(command.session);
      if (generation === this.#generation) { if (!uncertain) this.state.command = undefined; this.showError(error); }
    }
    finally { if (generation === this.#generation) this.emit(); }
  }
  async runDetail(id: string): Promise<void> {
    if (!this.state.session || !this.state.runs.some((run) => run.id === id)) return;
    const generation = this.#generation;
    try { const result = data(await this.api.request<{ data: RunDetail }>(`/workflows/runs/${encoded(id)}`, this.options())); if (generation === this.#generation) { this.state.details.set(id, result); this.emit(); } } catch (error) { if (generation === this.#generation) this.showError(error); }
  }
  async jobOutput(id: string): Promise<void> {
    if (!this.state.jobs.some((job) => job.id === id)) return;
    const generation = this.#generation; const session = this.state.session?.id;
    try { const output = data(await this.api.request<{ data: { text: string } }>(`/jobs/${encoded(id)}/output`, this.options())); if (generation !== this.#generation || this.state.session?.id !== session) return; this.state.notice = "Job output loaded."; this.jobLog = { id, text: output.text }; this.emit(); } catch (error) { if (generation === this.#generation) this.showError(error); }
  }
  jobLog: { id: string; text: string } | undefined;
  wake(): void { if (!this.state.authenticated) return; if (this.state.lease && this.state.lease.expires_at <= this.now()) { this.authenticationRequired("Browser attachment expired. Sign in to reconcile current work."); return; } if (!this.actionable) this.state.connection = "reconnecting"; this.emit(); void this.recover(this.#generation).catch((error) => this.showError(error)); }
  private options(): RequestOptions { return { directory: this.state.session?.directory ?? this.state.directory, signal: this.#attachment.signal }; }
  private async checkIdentity(): Promise<void> {
    const health = await this.api.request<{ id?: string }>("/health");
    if (this.#identity && health.id && health.id !== this.#identity) { this.authenticationRequired("Server restarted. Sign in again to obtain a fresh attachment."); throw new ApiFault("auth"); }
    this.#identity = health.id;
  }
  private async attachLease(generation: number): Promise<void> {
    const session = this.state.session; if (!session) return;
    this.#leaseKey ??= operationKey();
    const lease = data(await this.api.request<{ data: Lease }>(`/sessions/${encoded(session.id)}/interactive-clients`, { ...this.options(), method: "POST", body: {}, key: this.#leaseKey }));
    if (generation !== this.#generation) return;
    if (lease.session_id !== session.id || lease.expires_at <= this.now()) { this.#leaseKey = undefined; throw new ApiFault("changed"); }
    this.state.lease = lease; this.#leaseKey = undefined;
  }
  private scheduleRenewal(generation: number): void {
    clearTimeout(this.#renewal);
    this.#renewal = setTimeout(() => { void this.renew(generation); }, 15_000);
  }
  private async renew(generation: number): Promise<void> {
    const lease = this.state.lease; if (!lease || generation !== this.#generation) return;
    try {
      if (lease.expires_at <= this.now()) { this.authenticationRequired("Browser attachment expired. Sign in to reconcile current work."); return; }
      const renewed = data(await this.api.request<{ data: Lease }>(`/sessions/${encoded(lease.session_id)}/interactive-clients/${encoded(lease.client_id)}`, { ...this.options(), method: "PUT", body: {}, key: operationKey() }));
      if (generation !== this.#generation) return;
      this.state.lease = renewed; await this.refresh(); this.scheduleRenewal(generation);
    } catch (error) {
      if (generation !== this.#generation) return;
      if (error instanceof ApiFault && error.kind === "missing") this.authenticationRequired("Browser attachment expired or the server restarted. Sign in again.");
      else { this.showError(error); this.scheduleRenewal(generation); }
    }
  }
  private releaseLease(): void {
    const lease = this.state.lease; if (!lease) return;
    void this.api.request(`/sessions/${encoded(lease.session_id)}/interactive-clients/${encoded(lease.client_id)}`, { directory: this.state.session?.directory ?? this.state.directory, method: "DELETE", key: operationKey(), finishOnLogout: true }).catch(() => {});
    this.state.lease = undefined;
  }
  private stopAttachment(): void {
    this.#pageCursor = undefined; this.#pageVersion++; this.state.olderTranscript = false; this.state.messageCursor = undefined;
    this.#generation++; this.#attachment.abort(); this.#attachment = new AbortController();
    clearTimeout(this.#renewal); clearTimeout(this.#refreshTimer); this.#refreshing = undefined; this.#recovery = undefined; this.#networkRecovery = undefined; this.#opened.clear(); this.#leaseKey = undefined;
  }
  private startStreams(generation: number): void {
    const session = this.state.session; if (!session) return;
    void this.follow("durable", session.id, generation);
    void this.follow("live", session.id, generation);
  }
  private async follow(kind: string, session: string, generation: number): Promise<void> {
    const signal = this.#attachment.signal; let backoff = 500;
    while (!signal.aborted && generation === this.#generation) {
      try {
        const after = this.#cursors.get(session) ?? this.state.session?.seq ?? -1;
        const path = kind === "durable" ? `/sessions/${encoded(session)}/events?after=${after}` : "/event";
        for await (const event of this.api.stream(path, { directory: this.state.session!.directory, signal, opened: () => { if (generation === this.#generation) { this.#opened.add(kind); this.connected(); } } })) {
          if (generation !== this.#generation) return;
          backoff = 500; this.event(event, session);
        }
      } catch (error) {
        if (signal.aborted || generation !== this.#generation) return;
        this.#opened.delete(kind); this.showError(error); if (generation !== this.#generation) return; this.state.connection = backoff >= 4000 ? "offline" : "reconnecting"; this.emit();
      }
      await pause(backoff, signal); backoff = Math.min(15_000, backoff * 2);
      if (signal.aborted || generation !== this.#generation) return;
      try { await this.recover(generation); } catch (error) { this.showError(error); }
    }
  }
  private recover(generation: number): Promise<void> {
    if (this.#recovery) return this.#recovery;
    const work = (async () => {
      await this.checkIdentity(); if (generation !== this.#generation) return;
      if (this.state.lease && this.state.lease.expires_at <= this.now()) { this.authenticationRequired("Browser attachment expired. Sign in again."); return; }
      await this.refresh();
      if (!this.state.session) { await this.loadSessions(); if (generation === this.#generation) this.state.connection = "connected"; return; }
      if (!this.state.lease) await this.attachLease(generation);
      this.connected();
    })();
    this.#recovery = work;
    return work.finally(() => { if (this.#recovery === work) this.#recovery = undefined; });
  }
  private connected(): void { if (this.#opened.size === 2 && this.state.lease && this.state.lease.expires_at > this.now()) this.state.connection = "connected"; this.emit(); }
  private event(event: EventEnvelope, session: string): void {
    if (event.durable?.aggregateID === session) this.#cursors.set(session, Math.max(this.#cursors.get(session) ?? -1, event.durable.seq));
    const payload = event.data as Record<string, unknown> | null;
    if (event.type === "session.text.delta" && payload?.session_id === session && typeof payload.text === "string") { this.state.liveText += payload.text; this.emit(); return; }
    clearTimeout(this.#refreshTimer);
    this.#refreshTimer = setTimeout(() => { void this.refresh().catch((error) => this.showError(error)); }, 100);
  }
  private showError(error: unknown): void {
    if (error instanceof ApiFault && error.kind === "auth") { if (this.state.authenticated) this.authenticationRequired(); return; }
    this.state.notice = error instanceof ApiFault ? error.message : "This action could not be completed. Refresh current state before trying again.";
    if (!(error instanceof ApiFault) || error.kind === "network") { this.state.connection = "reconnecting"; if (this.state.authenticated) this.ensureRecovery(this.#generation); }
    this.emit();
  }
}
