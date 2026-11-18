/**
 * The chat panel controller (`editor-integration` → Chat panel Session binding, Extension
 * IDE token authentication, In-editor approvals, Editor context attachment). It owns the
 * connection: the IDE token lives only in the current `Link`, which is dropped on
 * disconnect, deactivation and every `401`. The webview receives state snapshots only.
 */
import * as vscode from "vscode";
import type { CyberClient } from "@cyber-code/sdk";
import { classify, confirmIdentity, createClient, describe, preflight, type Log } from "./api.js";
import { PendingTable, type Decision, type PendingRequest } from "./approvals.js";
import { answer, CHOICES, decide, QUESTION } from "./consent.js";
import { resolveExecutable, INSTALL_URL } from "./executable.js";
import { html, nonce } from "./html.js";
import { filePart, locationPath, selectedLines } from "./mention.js";
import { cyberState, readRegistration, startServer, transportFetch } from "./server.js";
import { expiresSoon, readTokenFile, tokenPath } from "./token.js";
import { chooseTransport, type Registration } from "./transport.js";
import { webviewMessage, withinLimit, type ChatMessage, type HostAction, type HostMessage, type ViewState, type WebviewMessage } from "../shared/protocol.js";

const WINDOW = 500;
const PAGE = 100;
const LEASE_MS = 15_000;
const FRAME_MS = 1000 / 60;
const SESSION_KEY = "cyber.boundSession";

interface Link {
  client: CyberClient;
  registration: Registration;
  close: () => Promise<void>;
  abort: AbortController;
}

export interface Hooks {
  /** Every message posted to the webview (tests only). */
  posted?: HostMessage[];
}

function emptyState(): ViewState {
  return {
    phase: "starting", notice: "", actions: [], folders: [], folder: 0, sessions: [], sessionId: "", mode: "", pendingMode: "",
    model: "", models: [], busy: false, messages: [], hasOlder: false, live: "", cards: [], chip: null,
  };
}

export class ChatController implements vscode.WebviewViewProvider {
  #view: vscode.WebviewView | undefined;
  #link: Link | undefined;
  #state: ViewState = emptyState();
  readonly table = new PendingTable();
  #connecting: Promise<void> | undefined;
  #unauthorized = 0;
  #asked = false;
  #olderCursor: string | undefined;
  #liveMessage = "";
  #lease: { id: string; session: string; timer: NodeJS.Timeout } | undefined;
  #emitTimer: NodeJS.Timeout | undefined;
  #refreshTimer: NodeJS.Timeout | undefined;
  #tokenWatch: NodeJS.Timeout | undefined;
  #notified = new Set<string>();
  #folderIndex = 0;
  onAnswered: (requestId: string, elsewhere: boolean) => void = () => {};
  openReview: (request: PendingRequest, location: string) => void = () => {};

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly log: Log,
    private readonly hooks: Hooks = {},
  ) {}

  get state(): ViewState {
    return this.#state;
  }

  get client(): CyberClient | undefined {
    return this.#link?.client;
  }

  get executable(): string | undefined {
    const configured = vscode.workspace.getConfiguration("cyber").inspect<string>("executablePath")?.globalValue;
    return resolveExecutable(configured, { platform: process.platform, path: process.env.PATH, pathExt: process.env.PATHEXT });
  }

  /** The bound Location: the selected workspace folder. */
  get location(): string {
    const folders = vscode.workspace.workspaceFolders ?? [];
    return folders[this.#folderIndex]?.uri.fsPath ?? folders[0]?.uri.fsPath ?? process.cwd();
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.#view = view;
    const roots = ["dist", "media"].map((dir) => vscode.Uri.joinPath(this.context.extensionUri, dir));
    view.webview.options = { enableScripts: true, enableCommandUris: false, enableForms: false, localResourceRoots: roots };
    view.webview.html = html({
      cspSource: view.webview.cspSource,
      nonce: nonce(),
      script: view.webview.asWebviewUri(vscode.Uri.joinPath(this.context.extensionUri, "dist", "webview.js")).toString(),
      style: view.webview.asWebviewUri(vscode.Uri.joinPath(this.context.extensionUri, "media", "panel.css")).toString(),
    });
    view.webview.onDidReceiveMessage((value) => void this.receive(value));
    view.onDidChangeVisibility(() => void this.visibility());
    view.onDidDispose(() => {
      this.#view = undefined;
      void this.releaseLease();
    });
    this.#folderIndex = this.activeFolder();
    void this.connect();
  }

  get visible(): boolean {
    return this.#view?.visible ?? false;
  }

  private activeFolder(): number {
    const uri = vscode.window.activeTextEditor?.document.uri;
    const folder = uri ? vscode.workspace.getWorkspaceFolder(uri) : undefined;
    return folder?.index ?? 0;
  }

  // ----- state and webview -----

  private set(patch: Partial<ViewState>): void {
    Object.assign(this.#state, patch);
    this.emit();
  }

  /** Coalesce state snapshots: at most one per animation frame (60 per second). */
  emit(): void {
    if (this.#emitTimer) return;
    this.#emitTimer = setTimeout(() => {
      this.#emitTimer = undefined;
      this.post({ type: "state", state: this.snapshot() });
    }, FRAME_MS);
  }

  private snapshot(): ViewState {
    const folders = (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.name);
    const state: ViewState = { ...this.#state, folders, folder: this.#folderIndex, cards: this.table.cards(), chip: this.chip() };
    while (!withinLimit({ type: "state", state }) && state.messages.length > 0) {
      state.messages = state.messages.slice(1);
      state.hasOlder = true;
    }
    return state;
  }

  post(message: HostMessage): void {
    this.hooks.posted?.push(message);
    void this.#view?.webview.postMessage(message);
  }

  private chip(): ViewState["chip"] {
    const editor = vscode.window.activeTextEditor;
    const enabled = vscode.workspace.getConfiguration("cyber").get<boolean>("includeEditorContext", true);
    if (!enabled || !editor || editor.document.uri.scheme !== "file" || editor.document.isUntitled) return null;
    const lines = selectedLines(editor.selection.start, editor.selection.end);
    const path = locationPath(this.location, editor.document.uri.fsPath);
    const label = lines ? `${path}#L${lines.start}-${lines.end}` : path;
    return { label, note: editor.document.isDirty ? "unsaved changes not included" : "" };
  }

  private phase(phase: ViewState["phase"], notice: string, actions: HostAction[] = []): void {
    this.set({ phase, notice, actions });
  }

  // ----- connection -----

  /** (Re)connect once at a time. */
  connect(): Promise<void> {
    this.#connecting ??= this.establish().finally(() => {
      this.#connecting = undefined;
    });
    return this.#connecting;
  }

  private async establish(): Promise<void> {
    await this.disconnect();
    if (!vscode.workspace.isTrusted) {
      this.phase("restricted", "Cyber Code is disabled in Restricted Mode. Trust this workspace to chat, start the server or answer approvals.", ["manageTrust"]);
      return;
    }
    const executable = this.executable;
    if (!executable) {
      this.phase("noCli", "Cyber Code CLI not found. Install cyber or set cyber.executablePath in your user settings.", ["installCli"]);
      return;
    }
    this.phase("connecting", "Connecting to the Cyber Code server…");
    const state = await cyberState();
    const registration = await this.registration(state, executable);
    if (!registration) return;
    await this.attach(state, registration);
  }

  /** The registered server, started with consent when none answers. */
  private async registration(state: string, executable: string): Promise<Registration | undefined> {
    const existing = await readRegistration(state);
    if (existing && (await this.answers(existing))) return existing;
    if (!(await this.mayStart())) {
      this.phase("noServer", "No Cyber Code server is running.", ["startServer"]);
      return undefined;
    }
    return this.startAndRead(state, executable);
  }

  private async startAndRead(state: string, executable: string): Promise<Registration | undefined> {
    try {
      await startServer(executable, this.location);
    } catch {
      this.phase("noServer", "cyber service start failed. Run it in a terminal to see why.", ["startServer"]);
      return undefined;
    }
    const registration = await readRegistration(state);
    if (!registration) this.phase("noServer", "The server started but did not register.", ["retry"]);
    return registration;
  }

  private async answers(registration: Registration): Promise<boolean> {
    const transport = chooseTransport(registration, process.platform);
    if (transport.kind === "refused") return true;
    const bound = transportFetch(transport);
    try {
      const response = await bound.fetch(`${bound.baseUrl}/api/v1/health`, { signal: AbortSignal.timeout(3000) });
      return response.ok;
    } catch {
      return false;
    } finally {
      void bound.close();
    }
  }

  /** `cyber.autoStartServer`: `always` starts, `never` waits for Start server, `ask` asks once. */
  private async mayStart(): Promise<boolean> {
    const config = vscode.workspace.getConfiguration("cyber");
    const decision = decide(config.inspect<string>("autoStartServer")?.globalValue, this.#asked);
    if (decision !== "ask") return decision === "start";
    this.#asked = true;
    const choice = await vscode.window.showInformationMessage(QUESTION, { modal: true }, ...CHOICES);
    const result = answer(choice);
    if (result.save) await config.update("autoStartServer", result.save, vscode.ConfigurationTarget.Global);
    return result.start;
  }

  private async attach(state: string, registration: Registration): Promise<void> {
    const transport = chooseTransport(registration, process.platform);
    if (transport.kind === "refused") {
      this.phase("refused", transport.message);
      return;
    }
    const bound = transportFetch(transport);
    const probe = await preflight(bound.fetch, bound.baseUrl, registration, transport.kind === "tcp");
    if (!probe.ok) {
      void bound.close();
      this.phase("incompatible", probe.message, ["retry"]);
      return;
    }
    const file = await readTokenFile(state);
    if (!file) {
      void bound.close();
      this.waitForToken(state);
      this.phase("noToken", "Create an IDE token to connect. The command runs in a terminal so you can see it.", ["createToken"]);
      return;
    }
    const client = createClient({ baseUrl: bound.baseUrl, fetch: bound.fetch, token: file.token, directory: this.location, log: this.log });
    this.#link = { client, registration, close: bound.close, abort: new AbortController() };
    await this.ready(registration, expiresSoon(file, Date.now()));
  }

  private async ready(registration: Registration, expiring: boolean): Promise<void> {
    const link = this.#link;
    if (!link) return;
    try {
      if (!(await confirmIdentity(link.client, registration))) {
        await this.disconnect();
        this.phase("incompatible", "The server answering is not the registered Cyber Code server.", ["retry"]);
        return;
      }
      this.#unauthorized = 0;
      const notice = expiring ? "Your IDE token expires within 7 days." : "";
      this.phase("ready", notice, expiring ? ["createToken"] : []);
      await Promise.all([this.loadSessions(), this.loadModels()]);
      await this.bindInitial();
    } catch (error) {
      await this.failed(error);
    }
  }

  async disconnect(): Promise<void> {
    await this.releaseLease();
    const link = this.#link;
    this.#link = undefined;
    if (!link) return;
    link.abort.abort();
    this.table.suspend();
    await link.close().catch(() => {});
  }

  /** A failed request: `401` drops the token and re-reads the file once; `403` scope is shown. */
  async failed(error: unknown): Promise<void> {
    const kind = classify(error);
    if (kind === "unauthorized") return this.unauthorized();
    if (kind === "incompatible") {
      this.phase("incompatible", "This Cyber Code server does not offer what the extension needs; upgrade cyber.", ["retry"]);
      return;
    }
    this.set({ notice: describe(error) });
  }

  private async unauthorized(): Promise<void> {
    await this.disconnect();
    this.#unauthorized++;
    if (this.#unauthorized >= 2) {
      this.#unauthorized = 0;
      this.waitForToken(await cyberState());
      this.phase("noToken", "The IDE token was refused. Create a new IDE token.", ["createToken"]);
      return;
    }
    await this.connect();
  }

  /** Reconnect when the token file appears or changes. */
  waitForToken(state: string): void {
    if (this.#tokenWatch) return;
    let before: string | undefined;
    void readTokenFile(state).then((file) => (before = file?.token));
    this.#tokenWatch = setInterval(() => {
      void readTokenFile(state).then((file) => {
        if (!file || file.token === before) return;
        clearInterval(this.#tokenWatch);
        this.#tokenWatch = undefined;
        void this.connect();
      });
    }, 1000);
  }

  createTokenInTerminal(): void {
    const executable = this.executable;
    if (!executable || !vscode.workspace.isTrusted) return;
    const terminal = vscode.window.createTerminal({ name: "Cyber Code: IDE token", shellPath: executable, shellArgs: ["ide", "token", "create", "--client", "vscode"], cwd: this.location });
    terminal.show();
    void cyberState().then((state) => this.waitForToken(state));
  }

  tokenFile(): Promise<string> {
    return cyberState().then(tokenPath);
  }

  // ----- Sessions -----

  private async loadSessions(): Promise<void> {
    const client = this.#link?.client;
    if (!client) return;
    const page = await client.request("v1.session.list", { query: { limit: 50 } });
    this.set({ sessions: rows<{ id: string; title: string }>(page).map((row) => ({ id: row.id, title: row.title })) });
  }

  private async loadModels(): Promise<void> {
    const client = this.#link?.client;
    if (!client) return;
    try {
      const models = (await client.request("v1.model.list")) as { data?: { id: string; available: boolean }[] };
      this.set({ models: (models.data ?? []).filter((m) => m.available).map((m) => m.id) });
    } catch (error) {
      this.log(`models unavailable: ${classify(error)}`);
    }
  }

  private async bindInitial(): Promise<void> {
    const remembered = this.context.workspaceState.get<string>(SESSION_KEY);
    const known = this.#state.sessions.find((session) => session.id === remembered) ?? this.#state.sessions[0];
    if (known) await this.bind(known.id);
  }

  async bind(sessionId: string): Promise<void> {
    const link = this.#link;
    if (!link) return;
    if (this.table.session !== sessionId) await this.releaseLease();
    link.abort.abort();
    link.abort = new AbortController();
    this.table.bind(sessionId);
    this.#liveMessage = "";
    await this.context.workspaceState.update(SESSION_KEY, sessionId);
    this.set({ sessionId, messages: [], live: "", hasOlder: false });
    const seq = await this.refreshSession();
    await Promise.all([this.loadMessages(), this.refreshPending()]);
    void this.follow(link, sessionId, seq);
    if (this.visible) await this.acquireLease();
  }

  /** The Session's mode, model and status; returns its durable seq. */
  private async refreshSession(): Promise<number | undefined> {
    const client = this.#link?.client;
    const id = this.#state.sessionId;
    if (!client || !id) return undefined;
    const session = envelope(await client.request("v1.session.get", { path: { sessionID: id } }));
    const info = (session.info ?? {}) as { mode?: string; model?: string };
    const pending = typeof session.pending_mode === "string" ? session.pending_mode : "";
    this.set({ mode: String(session.effective_mode ?? info.mode ?? ""), pendingMode: pending, model: info.model ?? "", busy: session.status !== "idle" });
    return typeof session.seq === "number" ? session.seq : undefined;
  }

  private async loadMessages(older = false): Promise<void> {
    const client = this.#link?.client;
    const id = this.#state.sessionId;
    if (!client || !id) return;
    const query: Record<string, string | number> = { order: "desc", limit: PAGE };
    if (older && this.#olderCursor) query.cursor = this.#olderCursor;
    const page = (await client.request("v1.message.list", { path: { sessionID: id }, query })) as unknown as { cursor?: { next?: string } };
    const loaded = rows<unknown>(page).map(toChat).filter((m): m is ChatMessage => m !== undefined).reverse();
    this.#olderCursor = page.cursor?.next;
    const messages = older ? [...loaded, ...this.#state.messages] : loaded;
    if (!older && this.#liveMessage && loaded.some((m) => m.id === this.#liveMessage)) this.set({ live: "" });
    this.set({ messages: messages.slice(-WINDOW), hasOlder: this.#olderCursor !== undefined });
  }

  async refreshPending(): Promise<void> {
    const client = this.#link?.client;
    if (!client) return;
    const [permissions, questions] = await Promise.all([client.request("v1.permission.list"), client.request("v1.question.list")]);
    const before = new Set(this.table.cards().map((card) => card.requestId));
    this.table.update([...(permissions as PendingRequest[]), ...(questions as PendingRequest[])]);
    for (const card of this.table.cards()) {
      if (card.state === "stale" && before.has(card.requestId)) this.onAnswered(card.requestId, true);
    }
    this.notifyHidden();
    this.emit();
  }

  /** A notification with Review for each new request while the panel is hidden. */
  private notifyHidden(): void {
    for (const card of this.table.cards()) {
      if (card.state !== "pending" || this.#notified.has(card.requestId)) continue;
      this.#notified.add(card.requestId);
      if (this.visible) continue;
      const what = card.kind === "permission" ? `${card.action} needs your approval` : "has a question";
      void vscode.window.showInformationMessage(`Cyber Code: ${what}.`, "Review").then((choice) => {
        if (choice === "Review") void vscode.commands.executeCommand("cyber.chat.focus");
      });
    }
  }

  /** Durable events from `seq` on, resuming across reconnects; deltas update the live text. */
  private async follow(link: Link, sessionId: string, seq: number | undefined): Promise<void> {
    const signal = link.abort.signal;
    try {
      for await (const event of link.client.sessions.events(sessionId, { after: seq, signal })) {
        if (signal.aborted) return;
        this.event(event.type, (event.data ?? {}) as Record<string, unknown>);
      }
    } catch (error) {
      if (!signal.aborted) await this.failed(error);
    }
  }

  private event(type: string, data: Record<string, unknown>): void {
    if (type === "session.text.delta" && typeof data.text === "string") {
      if (data.message_id !== this.#liveMessage) {
        this.#liveMessage = String(data.message_id ?? "");
        this.#state.live = "";
      }
      this.set({ live: this.#state.live + data.text });
      return;
    }
    if ((type === "permission.replied.1" || type === "question.replied.1") && typeof data.request_id === "string") {
      if (this.table.answered(data.request_id)) this.onAnswered(data.request_id, true);
    }
    clearTimeout(this.#refreshTimer);
    this.#refreshTimer = setTimeout(() => void this.refreshAll(), 100);
  }

  private async refreshAll(): Promise<void> {
    try {
      await Promise.all([this.refreshSession(), this.loadMessages(), this.refreshPending()]);
    } catch (error) {
      await this.failed(error);
    }
  }

  // ----- presence -----

  private async visibility(): Promise<void> {
    if (this.visible) {
      if (!this.#link && this.#state.phase !== "noToken") await this.connect();
      else await this.acquireLease();
      this.emit();
    } else {
      await this.releaseLease();
    }
  }

  private async acquireLease(): Promise<void> {
    const client = this.#link?.client;
    const session = this.#state.sessionId;
    if (!client || !session || this.#lease?.session === session) return;
    try {
      const lease = (await client.request("v1.session.interactiveAttach", { path: { sessionID: session } })) as { client_id: string };
      const timer = setInterval(() => void this.renewLease(), LEASE_MS);
      this.#lease = { id: lease.client_id, session, timer };
    } catch (error) {
      await this.failed(error);
    }
  }

  private async renewLease(): Promise<void> {
    const lease = this.#lease;
    const client = this.#link?.client;
    if (!lease || !client) return;
    try {
      await client.request("v1.session.interactiveRenew", { path: { sessionID: lease.session, clientID: lease.id } });
    } catch (error) {
      clearInterval(lease.timer);
      this.#lease = undefined;
      await this.failed(error);
    }
  }

  async releaseLease(): Promise<void> {
    const lease = this.#lease;
    this.#lease = undefined;
    if (!lease) return;
    clearInterval(lease.timer);
    await this.#link?.client
      .request("v1.session.interactiveRelease", { path: { sessionID: lease.session, clientID: lease.id } })
      .catch(() => {});
  }

  get leaseId(): string | undefined {
    return this.#lease?.id;
  }

  // ----- webview messages -----

  async receive(value: unknown): Promise<void> {
    const message = webviewMessage(value);
    if (!message) {
      this.log("dropped an invalid webview message");
      return;
    }
    try {
      await this.handle(message);
    } catch (error) {
      await this.failed(error);
    }
  }

  private async handle(message: WebviewMessage): Promise<void> {
    switch (message.type) {
      case "ready": return this.emit();
      case "prompt": return this.prompt(message.text, message.includeContext);
      case "interrupt": return this.interrupt();
      case "newSession": return this.newSession();
      case "selectSession": return this.bind(message.sessionId);
      case "selectFolder": return this.selectFolder(message.index);
      case "setMode": return this.setMode(message.mode);
      case "setModel": return this.setModel(message.model);
      case "reply": return void (await this.reply(message.sessionId, message.requestId, { reply: message.reply, ...(message.message !== undefined && { message: message.message }) }));
      case "answer": return void (await this.reply(message.sessionId, message.requestId, message.answers === undefined ? {} : { answers: message.answers }));
      case "review": return this.review(message.requestId);
      case "loadOlder": return this.loadMessages(true);
      case "openLink": return this.openLink(message.url);
      case "action": return this.action(message.action);
    }
  }

  private trustedLink(): Link | undefined {
    return vscode.workspace.isTrusted ? this.#link : undefined;
  }

  async prompt(text: string, includeContext: boolean): Promise<void> {
    const link = this.trustedLink();
    if (!link) return;
    if (!this.#state.sessionId) await this.newSession();
    const parts: unknown[] = [{ type: "text", text }];
    const context = includeContext ? this.contextPart() : undefined;
    if (context) parts.push(context);
    try {
      await link.client.request("v1.session.prompt", { path: { sessionID: this.#state.sessionId }, body: { parts, delivery: "steer" } as never });
      this.set({ notice: "", busy: true });
    } catch (error) {
      if (classify(error) === "unauthorized") return this.failed(error);
      this.set({ notice: describe(error) });
    }
  }

  /** The active editor as a `file` part, with the selection's lines. */
  private contextPart(): unknown {
    const editor = vscode.window.activeTextEditor;
    if (!editor || !vscode.workspace.getConfiguration("cyber").get<boolean>("includeEditorContext", true)) return undefined;
    if (editor.document.uri.scheme !== "file" || editor.document.isUntitled) return undefined;
    return filePart(this.location, editor.document.uri.fsPath, selectedLines(editor.selection.start, editor.selection.end), true);
  }

  private async interrupt(): Promise<void> {
    const link = this.trustedLink();
    if (link && this.#state.sessionId) await link.client.request("v1.session.interrupt", { path: { sessionID: this.#state.sessionId }, body: {} as never });
  }

  async newSession(): Promise<void> {
    const link = this.trustedLink();
    if (!link) return;
    const created = envelope(await link.client.request("v1.session.create", { body: {} as never }));
    const id = String(created.id);
    this.set({ sessions: [{ id, title: String(created.title ?? id) }, ...this.#state.sessions] });
    await this.bind(id);
  }

  private async selectFolder(index: number): Promise<void> {
    if (index >= (vscode.workspace.workspaceFolders?.length ?? 0)) return;
    this.#folderIndex = index;
    await this.context.workspaceState.update(SESSION_KEY, undefined);
    this.set({ sessionId: "", sessions: [], messages: [] });
    await this.connect();
  }

  private async setMode(mode: string): Promise<void> {
    const link = this.trustedLink();
    if (!link || !this.#state.sessionId) return;
    this.set({ pendingMode: mode });
    await link.client.request("v1.session.mode", { path: { sessionID: this.#state.sessionId }, body: { mode } as never });
    await this.refreshSession();
  }

  private async setModel(model: string): Promise<void> {
    const link = this.trustedLink();
    if (!link || !this.#state.sessionId) return;
    await link.client.request("v1.session.model", { path: { sessionID: this.#state.sessionId }, body: { model } as never });
    await this.refreshSession();
  }

  /**
   * Answer exactly `requestId` of `sessionId` if the host's own table holds it as pending
   * for the bound Session; anything else is dropped without a request.
   */
  async reply(sessionId: string, requestId: string, decision: Decision): Promise<boolean> {
    const link = this.trustedLink();
    const begun = link ? this.table.begin(sessionId, requestId, decision) : undefined;
    if (!link || !begun) {
      this.log("dropped a reply for a request that is not pending");
      this.emit();
      return false;
    }
    this.emit();
    const kind = begun.request.kind === "permission" ? "v1.permission.reply" : "v1.question.reply";
    try {
      await link.client.request(kind, { path: { sessionID: sessionId, requestID: requestId }, body: decision as never }, { idempotencyKey: begun.key });
      this.table.acknowledge(requestId);
      this.onAnswered(requestId, false);
      return true;
    } catch (error) {
      this.settleFailedReply(requestId, error);
      return false;
    } finally {
      this.emit();
    }
  }

  private settleFailedReply(requestId: string, error: unknown): void {
    const kind = classify(error);
    if (kind === "transport") this.table.unconfirmed(requestId);
    else if (kind === "unauthorized") {
      this.table.unconfirmed(requestId);
      void this.failed(error);
    } else if (error instanceof Error && /NotFound/.test(error.message)) {
      this.table.answered(requestId);
      this.onAnswered(requestId, true);
    } else {
      this.table.refused(requestId);
      this.set({ notice: describe(error) });
    }
  }

  private async review(requestId: string): Promise<void> {
    const request = this.table.get(requestId);
    if (request) this.openReview(request, this.location);
  }

  private async openLink(url: string): Promise<void> {
    let target: vscode.Uri;
    try {
      target = vscode.Uri.parse(url, true);
    } catch {
      return;
    }
    if (target.scheme !== "http" && target.scheme !== "https") return;
    const choice = await vscode.window.showWarningMessage(`Open ${url} in your browser?`, { modal: true }, "Open");
    if (choice === "Open") await vscode.env.openExternal(target);
  }

  private async action(action: HostAction): Promise<void> {
    switch (action) {
      case "createToken": return this.createTokenInTerminal();
      case "startServer": return this.startNow();
      case "manageTrust": return void vscode.commands.executeCommand("workbench.trust.manage");
      case "installCli": return void vscode.env.openExternal(vscode.Uri.parse(INSTALL_URL));
      case "saveDocument": return void vscode.window.activeTextEditor?.document.save().then(() => this.emit());
      case "retry": return this.connect();
      case "reviewChanges": return void vscode.commands.executeCommand("cyber.reviewSessionChanges");
      case "openTerminal": return void vscode.commands.executeCommand("cyber.openSessionInTerminal");
    }
  }

  /** The one-time `Start server` action: starts without changing the setting. */
  private async startNow(): Promise<void> {
    const executable = this.executable;
    if (!executable || !vscode.workspace.isTrusted) return;
    const registration = await this.startAndRead(await cyberState(), executable);
    if (registration) await this.connect();
  }

  insertMention(text: string): void {
    this.post({ type: "insert", text });
  }

  async dispose(): Promise<void> {
    clearInterval(this.#tokenWatch);
    clearTimeout(this.#emitTimer);
    clearTimeout(this.#refreshTimer);
    await this.disconnect();
  }
}

/** `{ data }` envelopes unwrapped (Location-scoped responses keep theirs). */
function envelope(value: unknown): Record<string, unknown> {
  const record = (value ?? {}) as Record<string, unknown>;
  const data = record.data;
  return typeof data === "object" && data !== null && !Array.isArray(data) && "id" in data ? (data as Record<string, unknown>) : record;
}

/** The rows of a page, whether or not it is wrapped in a Location envelope. */
function rows<T>(value: unknown): T[] {
  const data = (value as { data?: unknown } | undefined)?.data;
  if (Array.isArray(data)) return data as T[];
  const inner = (data as { data?: unknown } | undefined)?.data;
  return Array.isArray(inner) ? (inner as T[]) : [];
}

function toChat(raw: unknown): ChatMessage | undefined {
  const message = raw as { id?: string; kind?: string; text?: string; parts?: { type: string; text?: string }[]; tools?: { name: string; output?: string | null }[] };
  if (!message || typeof message.id !== "string") return undefined;
  if (message.kind === "user") {
    const text = (message.parts ?? []).map((part) => (part.type === "text" ? (part.text ?? "") : `[${part.type}]`)).join("\n");
    return { id: message.id, role: "user", text };
  }
  if (message.kind === "assistant") {
    const tools = (message.tools ?? []).map((tool) => `\n\n\`${tool.name}\``).join("");
    return { id: message.id, role: "assistant", text: `${message.text ?? ""}${tools}` };
  }
  return { id: message.id, role: "system", text: message.text ?? "" };
}
