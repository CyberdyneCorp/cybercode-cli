import { computerBanner, computerPanel } from "./computer-view.js";

import { integrationsView } from "./integrations-view.js";
import type { Content, Message, PendingRequest, RunDetail } from "../../sdk/typescript/src/generated/types.js";
import { Dashboard, type Card, type Tab } from "./model.js";
import { messageInbox } from "./inbox-view.js";
import { redactionNote } from "./redactions.js";

function node<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text?: string): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag); element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}
function button(label: string, action: () => void, disabled = false, className = "button"): HTMLButtonElement {
  const element = node("button", className, label); element.type = "button"; element.disabled = disabled; element.addEventListener("click", action); return element;
}
const text = (value: unknown) => typeof value === "string" ? value : JSON.stringify(value, null, 2) ?? "";
const money = (value: number) => Number.isFinite(value) ? `$${value.toFixed(3)} known priced subtotal` : "Cost unavailable";
const short = (value: string) => value.length > 20 ? `${value.slice(0, 9)}…${value.slice(-6)}` : value;
const prettyStatus = (value: string) => value.replaceAll("_", " ");
function badge(value: string, className = ""): HTMLElement { return node("span", `badge ${className}`, prettyStatus(value)); }
function code(value: unknown, label?: string): HTMLElement {
  const element = node("pre", "code", text(value)); if (label) element.setAttribute("aria-label", label); element.tabIndex = 0; return element;
}
function heading(eyebrow: string, title: string): HTMLElement { const wrap = node("div", "section-heading"); wrap.append(node("p", "eyebrow", eyebrow), node("h2", "", title)); return wrap; }
function labelInput(label: string, input: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement, key: string): HTMLElement {
  const wrap = node("label", "field"); wrap.append(node("span", "field-label", label), input); input.setAttribute("aria-label", label); input.dataset.focus = key; return wrap;
}

/** All untrusted content is textContent; no HTML, image, URL or script interpretation. */
export class View {
  constructor(readonly root: HTMLElement, readonly model: Dashboard) {}
  render(): void {
    const focus = document.activeElement as HTMLInputElement | null;
    const key = focus?.dataset.focus;
    const selection = focus && "selectionStart" in focus ? [focus.selectionStart, focus.selectionEnd] : undefined;
    const layout = this.model.state.authenticated ? this.dashboard() : this.login();
    this.root.replaceChildren(layout);
    if (key) {
      const replacement = this.root.querySelector<HTMLInputElement>(`[data-focus="${CSS.escape(key)}"]`);
      replacement?.focus({ preventScroll: true });
      if (selection && replacement && ["INPUT", "TEXTAREA"].includes(replacement.tagName)) {
        try { replacement.setSelectionRange(selection[0] ?? 0, selection[1] ?? 0); } catch { /* Select/radio controls do not expose text selection. */ }
      }
    }
    const dialog = this.root.querySelector<HTMLDialogElement>("dialog");
    if (dialog && !dialog.open) dialog.showModal();
    if (key && dialog) dialog.querySelector<HTMLElement>(`[data-focus="${CSS.escape(key)}"]`)?.focus({ preventScroll: true });
  }
  private login(): HTMLElement {
    const state = this.model.state;
    const main = node("main", "login-shell");
    const form = node("form", "login-card");
    const brand = node("div", "brand"); brand.append(node("span", "brand-mark", "C"), node("span", "", "CYBER"), badge("Local dashboard"));
    form.append(brand, node("p", "eyebrow", "YOUR MACHINE. YOUR CONTROL."), node("h1", "", "Your agents,\nwithin reach."), node("p", "lede", "Follow the work. Review the details. Keep the final say."));
    const authority = node("div", "authority"); authority.append(node("strong", "", "Owner access to this server"), node("p", "", "This password can control local Sessions, tools and approvals. It is not a limited device grant. Use a trusted HTTPS endpoint or your own SSH forward.")); form.append(authority);
    const password = node("input"); password.type = "password"; password.autocomplete = "off"; password.required = true; password.maxLength = 4096;
    form.append(labelInput("Password", password, "login-password"));
    const submit = node("button", "button primary wide", state.authBusy ? "Connecting…" : "Sign in"); submit.type = "submit"; submit.disabled = state.authBusy || Date.now() < state.retryAt; form.append(submit);
    if (state.retryAt > Date.now()) setTimeout(() => this.model.emit(), state.retryAt - Date.now() + 10);
    if (state.notice) { const message = node("p", "notice", state.notice); message.setAttribute("role", "status"); form.append(message); }
    const origin = node("p", "login-origin", window.location.origin); form.append(origin, node("p", "fine-print", "Password stays in page memory. Reloading signs you out. Closing this page does not stop the server."));
    form.addEventListener("submit", (event) => { event.preventDefault(); const value = password.value; password.value = ""; void this.model.login(value); });
    main.append(form, node("p", "login-footer", "PRIVATE WORKSPACE · NO ACCOUNT OR RELAY REQUIRED")); return main;
  }
  private dashboard(): HTMLElement {
    const state = this.model.state; const shell = node("div", "shell");
    const top = node("header", "topbar");
    const brand = node("div", "brand"); brand.append(node("span", "brand-mark", "C"), node("span", "", "CYBER"), node("span", "brand-divider", "/"), node("span", "top-label", "Workspace"));
    const connection = node("div", `connection ${state.connection === "connected" ? "online" : ""}`); connection.setAttribute("role", "status"); connection.setAttribute("aria-label", "Connection status"); connection.append(node("span", "status-dot"), node("span", "", prettyStatus(state.connection)));
    const right = node("div", "top-actions"); right.append(connection, button("Reconnect", () => this.model.wake(), state.connection === "connecting", "button quiet"), button("Logout", () => this.model.logout(), false, "button quiet"));
    top.append(brand, right);
    const workspace = node("div", "workspace"); workspace.append(this.sidebar(), this.main());
    shell.append(top, computerPanel(this.model.computer), workspace); if (state.review) shell.append(this.review(state.review)); if (state.feedback) shell.append(this.feedback(state.feedback)); return shell;
  }
  private sidebar(): HTMLElement {
    const state = this.model.state; const aside = node("aside", "sidebar");
    aside.append(heading("WORKSPACE", "Sessions"));
    const location = node("form", "location-form");
    const input = node("input"); input.type = "text"; input.value = state.directory; input.placeholder = "/path/to/project";
    location.append(labelInput("Location", input, "location"));
    const apply = node("button", "button location-button", "Open location"); apply.type = "submit"; apply.disabled = state.loading || !!state.creation; location.append(apply);
    location.addEventListener("submit", (event) => { event.preventDefault(); void this.model.switchLocation(input.value); }); aside.append(location);
    const list = node("nav", "session-list"); list.setAttribute("aria-label", "Sessions");
    for (const session of state.sessions) {
      const selected = session.id === state.session?.id;
      const item = button("", () => { void this.model.selectSession(session.id); }, false, `session-item ${selected ? "selected" : ""}`);
      item.setAttribute("aria-label", session.title || short(session.id)); item.setAttribute("aria-pressed", String(selected)); if (selected) item.setAttribute("aria-current", "page");
      item.append(node("span", "session-icon", selected ? "◈" : "◇"));
      const body = node("span", "session-item-body"); body.append(node("span", "session-title", session.title || "Untitled Session"), node("span", "session-subtitle", `${session.agent} · ${session.model}`)); item.append(body); list.append(item);
    }
    if (!state.sessions.length) list.append(node("p", "empty-small", "No Sessions in this Location. Create one with New Session."));
    if (state.sessionCursor) list.append(button("Load more Sessions", () => { void this.model.moreSessions(); }, state.loading, "button quiet wide"));
    aside.append(button(state.creation?.status === "uncertain" ? "Retry creating Session" : state.creation ? "Creating Session…" : "New Session", () => { void this.model.createSession(); }, state.connection !== "connected" || state.creation?.status === "submitting", "button wide"), list);
    const footer = node("div", "sidebar-footer"); footer.append(badge("Owner client"), node("p", "", "Execution stays on this server. Browser presence expires after 45 seconds without renewal.")); aside.append(footer); return aside;
  }
  private main(): HTMLElement {
    const state = this.model.state; const main = node("main", "main");
    const title = node("div", "session-header");
    const left = node("div"); left.append(node("p", "eyebrow", state.session?.directory ?? state.directory), node("h1", "session-heading", state.session?.title || "Your workspace"));
    const meta = node("div", "session-meta");
    if (state.session) meta.append(badge(state.session.status, state.session.status === "running" ? "accent" : ""), badge(state.session.effective_mode ?? state.session.mode), node("span", "muted mono", state.session.model));
    title.append(left, meta); main.append(title);
    const count = state.cards.length;
    const tabs = node("nav", "tabs"); tabs.setAttribute("aria-label", "Session views");
    for (const [tab, label] of [["conversation", "Conversation"], ["approvals", "Approvals"], ["runs", "Runs"], ["inbox", "Message inbox"], ["integrations", "Integrations"]] as const) {
      const item = button(label, () => this.model.setTab(tab), false, `tab ${state.tab === tab ? "active" : ""}`); item.setAttribute("aria-current", state.tab === tab ? "page" : "false"); if (tab === "approvals") item.append(node("span", "count", String(count))); tabs.append(item);
    }
    main.append(tabs);
    if (state.notice) { const notice = node("div", "notice", state.notice); notice.setAttribute("role", "status"); main.append(notice); }
    if (state.connection !== "connected") main.append(node("p", "stale-banner", "Live state is being reconciled. Approvals and execution controls are disabled until the connection and browser attachment are current."));
    if (state.command) {
      const command = node("div", "notice"); command.append(node("span", "", state.command.status === "submitting" ? `${state.command.description}: awaiting server acknowledgement…` : `${state.command.description}: result unknown. Retrying preserves its original request identity.`));
      if (state.command.status === "uncertain") command.append(button("Retry same action", () => { void this.model.retryCommand(); }, !this.model.actionable)); main.append(command);
    }
    const content = node("section", "content");
    if (state.loading && !state.session) content.append(node("p", "empty", "Loading authenticated Session state…"));
    else if (state.tab === "approvals") content.append(this.approvals());
    else if (state.tab === "integrations") content.append(integrationsView(this.model.integrations, this.model.actionable));
    else if (state.tab === "runs") content.append(this.runs());
    else if (state.tab === "inbox") content.append(messageInbox(this.model.inbox, this.model.actionable, (part) => this.part(part)));
    else content.append(this.conversation());
    main.append(content); if (state.session && state.tab === "conversation") main.append(this.composer()); return main;
  }
  private conversation(): HTMLElement {
    const state = this.model.state; const wrap = node("div", "conversation");
    if (state.olderTranscript) wrap.append(node("p", "muted", "Viewing an older transcript page. New messages are on the latest page."), button("Latest messages", () => { void this.model.latestMessages(); }, !this.model.actionable, "button quiet"));
    if (state.messageCursor) wrap.append(button("Load older messages", () => { void this.model.olderMessages(); }, !this.model.actionable, "button quiet"));
    if (!state.messages.length) {
      const empty = node("div", "empty-state"); empty.append(node("span", "empty-glyph", "✳"), node("h2", "", state.session ? "A clear place to start" : "Choose a Session"), node("p", "", state.session ? "Send a direction or queue the next step. Requests that need your judgement appear in Approvals." : "Open an existing Session to follow its work and review requests.")); wrap.append(empty);
    }
    for (const message of state.messages) wrap.append(this.message(message));
    if (state.liveText && !state.olderTranscript && !state.messages.some((message) => message.id === state.liveMessage)) { const live = node("article", "message assistant"); live.append(node("div", "message-label", "Assistant · streaming"), node("div", "prose", state.liveText)); wrap.append(live); }
    if (state.diffs.length) {
      const changes = node("section", "changes"); changes.append(heading("WORKING TREE", `${state.diffs.length} changed file${state.diffs.length === 1 ? "" : "s"}`));
      for (const diff of state.diffs) {
        const detail = node("details", "diff"); const summary = node("summary"); summary.append(node("span", "mono", diff.file), node("span", "diff-stats", `+${diff.additions} −${diff.deletions}`)); detail.append(summary, code(diff.patch, `Diff ${diff.file}`)); changes.append(detail);
      }
      wrap.append(changes);
    } else if (state.unavailable.has("Diffs")) wrap.append(node("p", "muted", "Diffs are unavailable on this server."));
    return wrap;
  }
  private message(message: Message): HTMLElement {
    const article = node("article", `message ${message.kind}`);
    const label = message.kind === "user" ? "You" : message.kind === "assistant" ? `${message.agent ?? "Assistant"} · ${message.finished ? "completed" : "working"}` : "Session note";
    article.append(node("div", "message-label", label));
    if (message.kind === "user") for (const part of message.parts) article.append(this.part(part));
    else {
      const live = this.model.state.liveMessage === message.id ? this.model.state.liveText : "";
      article.append(node("div", "prose", live.startsWith(message.text) && live.length > message.text.length ? live : message.text));
    }
    for (const tool of message.tools ?? []) {
      const detail = node("details", "tool"); const summary = node("summary", "", `${tool.name} · ${prettyStatus(tool.status)}`); const hidden = redactionNote(tool); if (hidden) summary.append(" ", node("span", "badge amber", hidden)); detail.append(summary, code(tool.arguments, "Tool arguments")); if (tool.output) detail.append(code(tool.output, "Tool output")); article.append(detail);
    }
    if (message.kind === "assistant" && message.error) article.append(node("p", "tool-error", "This step ended with an error. Review its tool output and the current Session state."));
    return article;
  }
  private part(part: Content): HTMLElement {
    if (part.type === "text") return node("div", "prose", part.text);
    if (part.type === "tool_result") return code(part.output, "Tool result");
    if (part.type === "tool_call") return code({ tool: part.name, input: part.input }, "Tool call");
    return node("p", "muted", ["image", "document", "attachment"].includes(part.type) ? "Attachment · display is outside this dashboard slice" : "Reasoning content is not expanded here.");
  }
  private composer(): HTMLElement {
    const state = this.model.state; const form = node("form", "composer");
    const input = node("textarea"); input.rows = 3; input.placeholder = "What should we work on next?"; input.value = state.draft; input.disabled = !this.model.actionable || !!state.command; input.maxLength = 128 * 1024;
    input.addEventListener("input", () => this.model.setDraft(input.value)); form.append(labelInput("Message", input, "composer"));
    const bar = node("div", "composer-bar"); const select = node("select");
    for (const [value, label] of [["steer", "Steer"], ["queue", "Queue"]]) { const option = node("option", "", label); option.value = value!; select.append(option); }
    select.value = state.delivery; select.disabled = !this.model.actionable || !!state.command; select.addEventListener("change", () => { state.delivery = select.value as "steer" | "queue"; }); bar.append(labelInput("Delivery", select, "delivery"));
    const actions = node("div", "composer-actions"); actions.append(button("Interrupt", () => { void this.model.interrupt(); }, !this.model.actionable || !!state.command || state.session?.status !== "running", "button quiet danger"));
    const send = node("button", "button primary", "Send message"); send.type = "submit"; send.disabled = !this.model.actionable || !!state.command; actions.append(send); bar.append(actions); form.append(bar, node("p", "fine-print", "Steer joins the current work; Queue waits for the next turn. Only a server acknowledgement confirms delivery."));
    form.addEventListener("submit", (event) => { event.preventDefault(); void this.model.prompt(); }); return form;
  }
  private approvals(): HTMLElement {
    const state = this.model.state; const wrap = node("div", "approvals");
    const intro = node("div", "inbox-intro"); intro.append(heading("HUMAN JUDGEMENT", "Approval inbox"), node("p", "muted", "Review the exact owner, origin and scope. A browser cannot revive a request already rejected while headless.")); wrap.append(intro);
    if (!state.cards.length) { const empty = node("div", "empty-state"); empty.append(node("span", "empty-glyph", "✓"), node("h2", "", "Nothing waiting for your decision"), node("p", "", "Requests appear here when the server asks. Your presence never grants permission by itself.")); wrap.append(empty); }
    for (const card of state.cards) wrap.append(card.request.kind === "permission" ? this.permission(card) : this.question(card));
    return wrap;
  }
  private requestHeader(card: Card): HTMLElement {
    const request = card.request; const header = node("header", "request-header");
    const title = request.kind === "permission" ? `Allow ${request.action}?` : "A question for you";
    header.append(badge(request.kind === "permission" ? "Permission" : "Question", "amber"), node("h3", "", title));
    const identity = node("dl", "identity");
    for (const [name, value] of [["Owner Session", request.session_id], ["Request", request.id], ["Origin", request.origin ? `${request.origin.title} · ${request.origin.agent}${request.origin.member ? ` · ${request.origin.member}` : ""}` : "This Session"], ["Location", request.origin?.directory ?? this.model.state.session?.directory ?? ""]]) { identity.append(node("dt", "", name), node("dd", "mono", value)); }
    header.append(identity); return header;
  }
  private status(card: Card): HTMLElement | undefined {
    if (!card.decision) return undefined;
    const status = node("div", "notice", card.decision.status === "submitting" ? "Waiting for server acknowledgement. No result is assumed." : "Answer result unknown. The original owner, request, displayed payload and idempotency key are retained.");
    if (card.decision.status === "uncertain") status.append(button("Retry same answer", () => { void this.model.answer(card, card.decision!.body, true); }, !this.model.actionable));
    return status;
  }
  private permission(card: Card): HTMLElement {
    const request = card.request; if (request.kind !== "permission") return node("div");
    const article = node("article", "request-card"); article.setAttribute("aria-label", `Permission request ${request.id}`); article.append(this.requestHeader(card));
    article.append(node("h4", "", "Requested resources"), code(request.resources.join("\n"), "Requested resources"));
    if (request.metadata !== undefined && request.metadata !== null) article.append(node("h4", "", "Command, diff and reason"), code(request.metadata, "Command or diff"));
    const actions = node("div", "request-actions"); const disabled = !this.model.actionable || !!card.decision;
    actions.append(button("Approve once", () => { void this.model.answer(card, { reply: "once" }); }, disabled, "button primary"));
    actions.append(button("Review saved approval", () => this.model.review(card), disabled || !request.always_patterns.length));
    actions.append(button("Reject and stop", () => { void this.model.answer(card, { reply: "reject" }); }, disabled, "button danger"));
    actions.append(button("Reject with feedback", () => this.model.feedback(card), disabled, "button quiet")); article.append(actions);
    const status = this.status(card); if (status) article.append(status); return article;
  }
  private question(card: Card): HTMLElement {
    const request = card.request; if (request.kind !== "question") return node("div");
    const article = node("article", "request-card"); article.setAttribute("aria-label", `Question request ${request.id}`); article.append(this.requestHeader(card));
    const answers = this.model.state.answers.get(card.key) ?? request.questions.map(() => []);
    const customs = this.model.state.customs.get(card.key) ?? request.questions.map(() => "");
    const disabled = !this.model.actionable || !!card.decision;
    request.questions.forEach((question, index) => {
      const fieldset = node("fieldset", "question"); fieldset.disabled = disabled; fieldset.append(node("legend", "", question.header), node("p", "", question.question));
      question.options.forEach((option, choice) => {
        const input = node("input"); input.type = question.multi_select ? "checkbox" : "radio"; input.name = `answer-${request.id}-${index}`; input.checked = answers[index]?.includes(option.label) ?? false; input.dataset.focus = `${card.key}-${index}-${choice}`;
        input.addEventListener("change", () => {
          const row = answers[index] ?? [];
          answers[index] = question.multi_select ? input.checked ? [...row, option.label] : row.filter((label) => label !== option.label) : [option.label];
          if (!question.multi_select) customs[index] = "";
          this.model.state.answers.set(card.key, answers); this.model.state.customs.set(card.key, customs); this.model.emit();
        });
        const label = node("label", "choice"); const caption = node("span"); caption.append(node("strong", "", option.label)); if (option.description) caption.append(node("span", "muted", option.description)); label.append(input, caption); fieldset.append(label);
      });
      if (question.allow_custom !== false) {
        const input = node("input"); input.type = "text"; input.value = customs[index] ?? ""; input.maxLength = 16 * 1024;
        input.addEventListener("input", () => { customs[index] = input.value; if (!question.multi_select) answers[index] = []; this.model.state.customs.set(card.key, customs); this.model.state.answers.set(card.key, answers); this.model.emit(); });
        fieldset.append(labelInput("Custom answer", input, `${card.key}-custom-${index}`));
      }
      article.append(fieldset);
    });
    const body = answers.map((row, index) => [...row, ...(customs[index]?.trim() ? [customs[index]!.trim()] : [])]);
    const actions = node("div", "request-actions"); actions.append(button("Submit answers", () => { void this.model.answer(card, { answers: body }); }, disabled || body.some((row) => !row.length), "button primary"), button("Dismiss question", () => { void this.model.answer(card, {}); }, disabled, "button quiet danger")); article.append(actions);
    const status = this.status(card); if (status) article.append(status); return article;
  }
  private nativeStop(dialog: HTMLDialogElement): void {
    const banner = computerBanner(this.model.computer); if (banner) dialog.prepend(banner);
  }
  private review(card: Card): HTMLDialogElement {
    const dialog = node("dialog", "dialog"); const request = card.request; if (request.kind !== "permission") return dialog;
    dialog.setAttribute("aria-labelledby", "saved-approval-heading"); const title = node("h2", "", "Review saved approval"); title.id = "saved-approval-heading";
    dialog.append(title, node("p", "", "This saves approval for the exact patterns below. Review their breadth and the command before confirming."), this.requestHeader(card), code(request.metadata ?? request.resources, "Command under review"), node("h3", "", "Saved-approval scope"), code(request.always_patterns.join("\n"), "Saved-approval scope"));
    const input = node("input"); input.type = "checkbox"; input.dataset.focus = "review-scope"; input.checked = this.model.state.reviewed; input.addEventListener("change", () => { this.model.state.reviewed = input.checked; this.model.emit(); });
    const label = node("label", "choice"); label.append(input, node("span", "", "I reviewed this saved approval scope")); dialog.append(label);
    const actions = node("div", "request-actions"); actions.append(button("Cancel", () => { this.model.state.review = undefined; this.model.emit(); }), button("Approve always", () => { void this.model.answer(card, { reply: "always" }); }, !this.model.state.reviewed || !this.model.actionable, "button primary")); dialog.append(actions);
    this.nativeStop(dialog);
    dialog.addEventListener("cancel", (event) => { event.preventDefault(); this.model.state.review = undefined; this.model.emit(); }); return dialog;
  }
  private feedback(card: Card): HTMLDialogElement {
    const dialog = node("dialog", "dialog"); dialog.setAttribute("aria-labelledby", "feedback-heading"); const title = node("h2", "", "Reject with feedback"); title.id = "feedback-heading";
    dialog.append(title, node("p", "", "The model receives your feedback. Reject and stop, without feedback, is a separate action."), this.requestHeader(card));
    const input = node("textarea"); input.rows = 4; input.maxLength = 16 * 1024; input.value = this.model.state.feedbackDraft; dialog.append(labelInput("Rejection feedback", input, "rejection-feedback"));
    const submit = button("Reject with feedback", () => { if (input.value.trim()) void this.model.answer(card, { reply: "reject", message: input.value }); }, true, "button danger");
    input.addEventListener("input", () => { this.model.state.feedbackDraft = input.value; submit.disabled = !input.value.trim() || !this.model.actionable; });
    const actions = node("div", "request-actions"); actions.append(button("Cancel", () => { this.model.state.feedback = undefined; this.model.emit(); }), submit); dialog.append(actions);
    this.nativeStop(dialog);
    dialog.addEventListener("cancel", (event) => { event.preventDefault(); this.model.state.feedback = undefined; this.model.emit(); }); return dialog;
  }
  private reservations(): HTMLElement {
    const state = this.model.state;
    const allocations = node("section", "run-section"); allocations.append(node("h3", "", "Budget allocations"));
    if (state.reservationError) allocations.append(node("p", "muted", state.reservationError));
    else if (!state.reservations.length) allocations.append(node("p", "empty-small", "No request reservations for this Session. Soft budgets do not reserve capacity."));
    for (const allocation of state.reservations) {
      const card = node("details", "run-card");
      card.append(node("summary", "", `${allocation.purpose} · ${allocation.state} · ${allocation.held_tokens.toLocaleString()} tokens held`), node("p", "fine-print", "Declared request bounds are scheduling estimates. Uncertain requests keep their remaining allocation until an owner audits the provider outcome."), code(allocation, "Reservation bounds, actual usage and uncertainty")); allocations.append(card);
    }
    return allocations;
  }
  private runs(): HTMLElement {
    const state = this.model.state; const wrap = node("div", "runs"); const disabled = !this.model.actionable || !!state.command;
    wrap.append(heading("IN FLIGHT & RECENT", "Work monitor"), node("p", "fine-print", "Costs are known priced subtotals. Unknown pricing and incomplete usage may add cost; zero does not establish free execution."));
    wrap.append(this.reservations());
    const workflows = node("section", "run-section"); workflows.append(node("h3", "", "Workflow runs"));
    if (state.unavailable.has("Workflows")) workflows.append(node("p", "muted", "Workflow routes are unavailable on this server."));
    else if (!state.runs.length) workflows.append(node("p", "empty-small", "No workflow runs for this Session."));
    for (const run of state.runs) {
      const card = node("article", "run-card"); card.append(node("h4", "", run.name), badge(run.status), node("p", "muted", `${run.phase ?? "No current phase"} · ${money(run.cost_usd)} · ${run.tokens.toLocaleString()} tokens`));
      const actions = node("div", "request-actions");
      if (["running", "waiting_input", "queued"].includes(run.status)) actions.append(button("Pause workflow", () => { void this.model.control(`/workflows/runs/${encodeURIComponent(run.id)}/pause`, "Pause workflow"); }, disabled));
      if (run.status === "paused") actions.append(button("Resume workflow", () => { void this.model.control(`/workflows/runs/${encodeURIComponent(run.id)}/resume`, "Resume workflow"); }, disabled));
      if (["running", "waiting_input", "paused", "queued"].includes(run.status)) actions.append(button("Stop workflow", () => { void this.model.control(`/workflows/runs/${encodeURIComponent(run.id)}/stop`, "Stop workflow"); }, disabled, "button danger"));
      actions.append(button("View run evidence", () => { void this.model.runDetail(run.id); }, !this.model.actionable, "button quiet")); card.append(actions);
      const detail = state.details.get(run.id); if (detail) card.append(this.evidence(detail)); workflows.append(card);
    }
    wrap.append(workflows);
    const jobs = node("section", "run-section"); jobs.append(node("h3", "", "Background Jobs"));
    if (state.unavailable.has("Jobs")) jobs.append(node("p", "muted", "Job routes are unavailable on this server."));
    else if (!state.jobs.length) jobs.append(node("p", "empty-small", "No background Jobs for this Session."));
    for (const job of state.jobs) {
      const card = node("article", "run-card"); card.append(node("h4", "", job.name || job.description), badge(job.status), node("p", "muted", `${job.kind} · ${money(job.cost)}`)); if (job.command) card.append(code(job.command, "Job command"));
      const actions = node("div", "request-actions"); if (job.status === "running") actions.append(button("Stop Job", () => { void this.model.control(`/jobs/${encodeURIComponent(job.id)}/stop`, "Stop Job"); }, disabled, "button danger"));
      actions.append(button("View Job output", () => { void this.model.jobOutput(job.id); }, !this.model.actionable, "button quiet")); card.append(actions); if (this.model.jobLog?.id === job.id) card.append(code(this.model.jobLog.text, "Job output")); jobs.append(card);
    }
    wrap.append(jobs);
    const goals = node("section", "run-section"); goals.append(node("h3", "", "Goals"));
    if (state.unavailable.has("Goals")) goals.append(node("p", "muted", "Goal routes are unavailable on this server."));
    else if (!state.goals?.current) goals.append(node("p", "empty-small", "No current Goal. Browser presence does not start one."));
    const goal = state.goals?.current;
    if (goal && state.session) {
      const card = node("article", "run-card"); card.append(node("h4", "", goal.condition), badge(goal.status)); if (goal.reason) card.append(node("p", "prose", goal.reason));
      const base = `/sessions/${encodeURIComponent(state.session.id)}/goals`;
      const actions = node("div", "request-actions"); if (goal.status === "active") actions.append(button("Pause current Goal", () => { void this.model.control(`${base}/pause`, "Pause Goal"); }, disabled)); if (goal.status === "paused") actions.append(button("Resume current Goal", () => { void this.model.control(`${base}/resume`, "Resume Goal"); }, disabled)); actions.append(button("Clear Goal", () => { void this.model.control(`${base}/${encodeURIComponent(goal.goal_id)}`, "Clear Goal", "DELETE"); }, disabled, "button danger")); card.append(actions); goals.append(card);
    }
    wrap.append(goals); return wrap;
  }
  private evidence(detail: RunDetail): HTMLElement {
    const wrap = node("section", "evidence"); wrap.append(node("h4", "", "Run evidence"), node("p", "muted", `${detail.status}${detail.reason ? ` · ${detail.reason}` : ""}`));
    const evidence = detail.solve_evidence;
    if (!evidence) { wrap.append(node("p", "muted", "No server-owned solve evidence attached. Workflow output is not a verification badge.")); return wrap; }
    const report = detail.result && typeof detail.result === "object" ? detail.result as Record<string, unknown> : {};
    const recommended = typeof report.recommended_candidate === "string" ? report.recommended_candidate : undefined;
    const known = evidence.candidates.find((candidate) => candidate.label === recommended || candidate.candidate_id === recommended);
    if (detail.result === undefined && detail.result_file) {
      wrap.append(node("p", "evidence-recommendation", "Recommendation unavailable: report stored externally. This view is incomplete."), code(detail.result_file, "External report path"));
      if (detail.result_preview) wrap.append(code(detail.result_preview, "External report preview"));
    } else {
      wrap.append(node("p", "evidence-recommendation", known?.host_verified && report.verification_status === "recommended" ? `Workflow recommendation: ${known.label ?? known.candidate_id} · review required` : "No candidate recommended. Review checks and incomplete outcomes."));
    }
    if (report.budget !== undefined) { const budget = node("details", "tool"); budget.append(node("summary", "", "Budget observation and completeness"), code(report.budget, "Budget observation")); wrap.append(budget); }
    if (report.limitations !== undefined) wrap.append(code(report.limitations, "Report limitations"));
    if (typeof report.verification_status === "string") wrap.append(badge(report.verification_status));
    for (const candidate of evidence.candidates) {
      const card = node("div", "candidate"); card.append(node("h4", "", candidate.label ?? candidate.candidate_id), badge(candidate.host_verified ? "Host-verified check evidence" : "Unverified"), node("p", "mono", candidate.branch ?? "No retained branch"), node("p", "muted", `${candidate.model ?? "Model unavailable"} · ${money(candidate.cost_usd)}`), code(candidate.checks, "Candidate checks")); wrap.append(card);
    }
    wrap.append(node("p", "fine-print", "Recommendations require human review. This dashboard does not apply, merge or publish patches.")); return wrap;
  }
}
