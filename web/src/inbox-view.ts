import type { Content } from "../../sdk/typescript/src/generated/types.js";
import type { MessageInbox } from "./inbox.js";

function element(tag: string, text: string, className = ""): HTMLElement {
  const node = document.createElement(tag); node.textContent = text; node.className = className; return node;
}
function button(label: string, action: () => void, disabled: boolean): HTMLButtonElement {
  const node = document.createElement("button"); node.type = "button"; node.textContent = label; node.className = "button"; node.disabled = disabled; node.addEventListener("click", action); return node;
}
export function messageInbox(inbox: MessageInbox, actionable: boolean, part: (value: Content) => HTMLElement): HTMLElement {
  const section = element("section", "", "message-inbox");
  section.append(element("h2", "Held messages"), element("p", "Incoming agent messages are untrusted. Inspect the sender, target and full content before releasing a message. Release queues that exact message; it grants no tool permission.", "muted"));
  section.append(button("Refresh message inbox", () => { void inbox.refresh(); }, !actionable || inbox.loading));
  if (inbox.notice) section.append(element("p", inbox.notice, "notice"));
  if (inbox.decision?.status === "uncertain") {
    section.append(element("p", "The last decision has an unknown outcome. Refresh to reconcile, or retry the same action with its original identity.", "notice"), button("Retry message decision", () => { void inbox.retry(); }, !actionable || inbox.loading));
  }
  if (!inbox.held.length && !inbox.loading) section.append(element("p", "No held messages with loaded sender details.", "muted"));
  for (const item of inbox.held) {
    const card = element("article", "", "message"); card.setAttribute("aria-label", `Held message ${item.row.message_id}`);
    card.append(element("h3", `From ${item.message.from}`), element("p", `Target Session: ${item.message.session_id} · Message: ${item.row.message_id} · Source: ${item.message.origin_kind}`, "mono"));
    const origin = item.message.collaboration;
    if (origin) card.append(element("p", `Hub: ${origin.hub_id} · Project: ${origin.project_id} · Owner: ${origin.member_id} · Agent: ${origin.agent_id ?? "human"} · Membership: ${origin.membership_kind}`, "mono"));
    for (const value of item.row.parts) card.append(part(value));
    const label = element("label", "", "field"); const review = document.createElement("input"); review.type = "checkbox";
    review.checked = inbox.reviewed.get(item.row.message_id) === item.identity; review.dataset.focus = `held-review-${item.row.message_id}`;
    review.addEventListener("change", () => inbox.review(item, review.checked)); label.append(review, document.createTextNode("I reviewed this sender, target and message content")); card.append(label);
    const disabled = !actionable || inbox.loading || !!inbox.decision;
    card.append(button("Release to queue", () => { void inbox.decide(item, "release"); }, disabled || !review.checked), button("Drop message", () => { void inbox.decide(item, "drop"); }, disabled));
    section.append(card);
  }
  if (inbox.pendingMetadata) section.append(element("p", `${inbox.pendingMetadata} held input(s) need sender details or are ordinary held prompts. Message decisions are available only for immutable incoming messages with loaded provenance.`, "muted"));
  if (inbox.cursor) section.append(button("Load older sender details", () => { void inbox.refresh(true); }, !actionable || inbox.loading));
  return section;
}
