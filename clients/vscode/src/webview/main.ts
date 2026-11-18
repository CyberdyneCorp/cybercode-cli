/**
 * The chat panel. It holds no credential and makes no network request: it renders the
 * host's state with text nodes and the inert Markdown tree, and posts validated messages
 * back. Links are shown with their URL and opened by the host after a confirmation.
 */
import { hostMessage, MODES, type Card, type HostAction, type ViewState, type WebviewMessage } from "../shared/protocol.js";
import { render, type Node } from "./render.js";

declare function acquireVsCodeApi(): { postMessage(message: unknown): void };

const vscode = acquireVsCodeApi();
const post = (message: WebviewMessage) => vscode.postMessage(message);

const ACTION_LABELS: Record<HostAction, string> = {
  createToken: "Create IDE token",
  startServer: "Start server",
  manageTrust: "Manage Workspace Trust",
  installCli: "Install instructions",
  saveDocument: "Save",
  retry: "Retry",
  reviewChanges: "Review Session changes",
  openTerminal: "Open in terminal",
};

function h<K extends keyof HTMLElementTagNameMap>(tag: K, props: { className?: string; text?: string; title?: string } = {}, children: (Element | string)[] = []): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  if (props.className) element.className = props.className;
  if (props.text !== undefined) element.textContent = props.text;
  if (props.title) element.title = props.title;
  for (const child of children) element.append(child);
  return element;
}

function button(label: string, onClick: () => void, options: { disabled?: boolean; className?: string } = {}): HTMLButtonElement {
  const element = h("button", { text: label, className: options.className ?? "" });
  element.type = "button";
  element.disabled = options.disabled ?? false;
  element.addEventListener("click", onClick);
  return element;
}

function select(options: { value: string; label: string }[], current: string, onChange: (value: string) => void, disabled = false): HTMLSelectElement {
  const element = h("select");
  for (const option of options) {
    const item = h("option", { text: option.label });
    item.value = option.value;
    item.selected = option.value === current;
    element.append(item);
  }
  element.disabled = disabled;
  element.addEventListener("change", () => onChange(element.value));
  return element;
}

/** DOM for the inert Markdown tree: elements from the allowlist and text nodes only. */
function toDom(node: Node): globalThis.Node {
  if (typeof node === "string") return document.createTextNode(node);
  if (node.tag === "a") {
    const href = node.href ?? "";
    const link = h("span", { className: "link", title: href });
    for (const child of node.children) link.append(toDom(child));
    link.append(h("span", { className: "url", text: ` (${href})` }));
    link.tabIndex = 0;
    link.addEventListener("click", () => post({ type: "openLink", url: href }));
    return link;
  }
  const element = document.createElement(node.tag);
  for (const child of node.children) element.append(toDom(child));
  return element;
}

const app = document.getElementById("app") as HTMLElement;
const transcript = h("section", { className: "transcript" });
const cards = h("section", { className: "cards" });
const header = h("header");
const notice = h("div", { className: "notice" });
const chip = h("div", { className: "chip" });
const composer = h("textarea");
composer.rows = 3;
composer.placeholder = "Ask Cyber Code";
let includeContext = true;
let state: ViewState | undefined;

function send(): void {
  const text = composer.value.trim();
  if (!text || state?.phase !== "ready") return;
  post({ type: "prompt", text, includeContext });
  composer.value = "";
}

composer.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    send();
  }
});

const sendButton = button("Send", send);
const stopButton = button("Interrupt", () => post({ type: "interrupt" }));
app.append(header, notice, transcript, cards, h("footer", {}, [chip, composer, h("div", { className: "row" }, [sendButton, stopButton])]));

function renderHeader(view: ViewState): void {
  header.replaceChildren();
  if (view.folders.length > 1) {
    header.append(select(view.folders.map((folder, index) => ({ value: String(index), label: folder })), String(view.folder), (value) => post({ type: "selectFolder", index: Number(value) })));
  }
  if (view.phase !== "ready") return;
  const sessions = view.sessions.map((session) => ({ value: session.id, label: session.title || session.id }));
  header.append(select(sessions, view.sessionId, (value) => post({ type: "selectSession", sessionId: value })), button("New", () => post({ type: "newSession" })));
  const modes = MODES.map((mode) => ({ value: mode, label: mode }));
  const modeLabel = view.pendingMode ? `${view.pendingMode} (pending)` : view.mode;
  header.append(select(modes, view.pendingMode || view.mode, (value) => post({ type: "setMode", mode: value as (typeof MODES)[number] })), h("span", { className: "mode", text: modeLabel }));
  if (view.models.length > 0) {
    const models = view.models.map((model) => ({ value: model, label: model }));
    header.append(select(models, view.model, (value) => post({ type: "setModel", model: value })));
  }
}

function renderNotice(view: ViewState): void {
  notice.replaceChildren();
  if (view.notice) notice.append(h("p", { text: view.notice }));
  for (const action of view.actions) notice.append(button(ACTION_LABELS[action], () => post({ type: "action", action })));
}

function renderTranscript(view: ViewState): void {
  transcript.replaceChildren();
  if (view.hasOlder) transcript.append(button("Load older", () => post({ type: "loadOlder" })));
  for (const message of view.messages) {
    const body = h("div", { className: `message ${message.role}` });
    if (message.role === "assistant") for (const node of render(message.text)) body.append(toDom(node));
    else body.append(h("pre", { text: message.text }));
    transcript.append(body);
  }
  if (view.live) {
    const live = h("div", { className: "message assistant live" });
    for (const node of render(view.live)) live.append(toDom(node));
    transcript.append(live);
  }
}

function renderCard(card: Card): HTMLElement {
  const disabled = card.state === "stale" || card.state === "sending";
  const element = h("article", { className: `card ${card.state}` });
  const title = card.kind === "permission" ? `${card.action}: ${card.resources.join(", ")}` : "Question";
  element.append(h("h4", { text: title }));
  if (card.origin) element.append(h("p", { className: "origin", text: `From Session ${card.origin}` }));
  if (card.taint) element.append(h("pre", { className: "taint", text: card.taint }));
  if (card.detail) element.append(h("pre", { className: "detail", text: card.detail }));
  if (card.state === "unconfirmed") element.append(h("p", { text: "Reply sent, not confirmed. Choose the same answer to retry." }));
  if (card.state === "stale") element.append(h("p", { text: "Answered elsewhere or no longer pending." }));
  if (card.kind === "permission") element.append(permissionActions(card, disabled));
  else element.append(questionActions(card, disabled));
  return element;
}

function permissionActions(card: Card, disabled: boolean): HTMLElement {
  const reply = (value: "once" | "always" | "reject", message?: string) =>
    post({ type: "reply", sessionId: card.sessionId, requestId: card.requestId, reply: value, ...(message !== undefined && { message }) });
  const row = h("div", { className: "row" });
  if (card.hasDiff) row.append(button("Review diff", () => post({ type: "review", requestId: card.requestId }), { disabled }));
  row.append(button("Accept", () => reply("once"), { disabled }));
  if (card.alwaysPatterns.length > 0) {
    row.append(button(`Accept always (${card.alwaysPatterns.join(", ")})`, () => reply("always"), { disabled }));
  }
  row.append(button("Reject", () => reply("reject"), { disabled }));
  const feedback = h("input");
  feedback.placeholder = "Feedback";
  row.append(feedback, button("Reject with feedback", () => reply("reject", feedback.value), { disabled }));
  row.append(h("p", { className: "hint", text: "Rejecting also declines the Session's other pending requests." }));
  return row;
}

function questionActions(card: Card, disabled: boolean): HTMLElement {
  const box = h("div");
  const chosen: string[][] = card.questions.map(() => []);
  card.questions.forEach((question, index) => {
    box.append(h("p", { text: `${question.header}: ${question.question}` }));
    for (const option of question.options) {
      box.append(button(option, () => {
        chosen[index] = question.multiSelect ? [...(chosen[index] ?? []), option] : [option];
        if (!question.multiSelect && card.questions.length === 1) post({ type: "answer", sessionId: card.sessionId, requestId: card.requestId, answers: chosen });
      }, { disabled }));
    }
  });
  if (card.questions.length > 1 || card.questions.some((q) => q.multiSelect)) {
    box.append(button("Submit", () => post({ type: "answer", sessionId: card.sessionId, requestId: card.requestId, answers: chosen }), { disabled }));
  }
  box.append(button("Dismiss", () => post({ type: "answer", sessionId: card.sessionId, requestId: card.requestId }), { disabled }));
  return box;
}

function renderChip(view: ViewState): void {
  chip.replaceChildren();
  if (!view.chip || !includeContext) return;
  chip.append(h("span", { text: view.chip.label }));
  if (view.chip.note) chip.append(h("span", { className: "note", text: ` ${view.chip.note}` }), button("Save", () => post({ type: "action", action: "saveDocument" })));
  chip.append(button("×", () => {
    includeContext = false;
    if (state) renderChip(state);
  }, { className: "remove" }));
}

function show(view: ViewState): void {
  state = view;
  renderHeader(view);
  renderNotice(view);
  renderTranscript(view);
  cards.replaceChildren(...view.cards.map(renderCard));
  renderChip(view);
  const ready = view.phase === "ready";
  composer.disabled = !ready;
  sendButton.disabled = !ready;
  stopButton.disabled = !ready || !view.busy;
}

window.addEventListener("message", (event: MessageEvent) => {
  const message = hostMessage(event.data);
  if (!message) return;
  if (message.type === "state") show(message.state);
  else if (message.type === "insert") {
    includeContext = true;
    composer.value = composer.value ? `${composer.value} ${message.text} ` : `${message.text} `;
    composer.focus();
  }
});

post({ type: "ready" });
