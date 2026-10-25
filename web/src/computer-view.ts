import type { ComputerControls, NativeReport } from "./computer.js";
function node<K extends keyof HTMLElementTagNameMap>(tag: K, text = ""): HTMLElementTagNameMap[K] { const element = document.createElement(tag); element.textContent = text; return element; }
function button(label: string, action: () => void, disabled = false): HTMLButtonElement { const element = node("button", label); element.type = "button"; element.className = "button"; element.disabled = disabled; element.addEventListener("click", action); return element; }
function checkbox(label: string, checked: boolean, action: (value: boolean) => void): HTMLElement { const wrap = node("label"); const input = node("input"); input.type = "checkbox"; input.checked = checked; input.addEventListener("change", () => action(input.checked)); wrap.append(input, document.createTextNode(label)); return wrap; }

/** Inert text only: native target names never become HTML or executable URLs. */
export function computerPanel(model: ComputerControls): HTMLElement {
  const section = node("section"); section.className = "authority computer-controls"; section.setAttribute("aria-label", "Native computer controls");
  const banner = controlBanner(model); if (banner) section.append(banner);
  const details = node("details"); details.open = model.expanded; details.addEventListener("toggle", () => { model.expanded = details.open; }); details.append(node("summary", "Native computer use (experimental)")); section.append(details);
  const report = model.report;
  if (!report) { details.append(node("p", "Native health is unavailable."), button("Refresh native status", () => { void model.refresh(); })); return section; }
  healthControls(details, model, report);
  captureEvidence(details, model);
  targetReview(details, model);
  if (model.notice) { const notice = node("p", model.notice); notice.setAttribute("role", "status"); details.append(notice); }
  return section;
}
function controlBanner(model: ComputerControls): HTMLElement | undefined {
  const active = model.active; if (!active && !model.uncertain) return undefined;
  const banner = node("div"); banner.className = "notice"; banner.setAttribute("role", "alert");
  if (active) { banner.append(node("strong", `Computer ${model.uncertain ? "status unknown" : "ARMED"}: ${active.target.identity}`), node("p", `Session ${active.session_id} · expires ${new Date(active.expires_at).toLocaleTimeString()}`)); }
  else { banner.append(node("p", "Native computer status is unknown. Confirm Stop on the host before continuing.")); }
  banner.append(button("STOP native input", () => { void model.stop(); })); return banner;
}
function healthControls(details: HTMLElement, model: ComputerControls, report: NativeReport): void {
  const health = report.health;
  const message = !report.enabled ? "Disabled. Enable computer_use in trusted global configuration and restart the server." : !health.supported ? `Unsupported: requires ${health.minimum_version}; host is ${health.platform}.` : `Screen Recording: ${health.screen_capture ? "granted" : "missing"}; Accessibility: ${health.accessibility ? "granted" : "missing"}.`;
  details.append(node("p", message), node("p", `Running server binary: ${health.binary}`));
  if (!report.enabled || !health.supported) return;
  details.append(button("Request OS consent (opens prompts on the host)", () => { void model.setup(); }, model.busy));
  const input = node("input"); input.value = model.selector; input.placeholder = "com.apple.TextEdit"; input.setAttribute("aria-label", "Application bundle ID or display ID"); input.dataset.focus = "computer-target"; input.addEventListener("input", () => { model.selector = input.value; });
  details.append(input, checkbox("Explicit whole display scope", model.display, (value) => { model.display = value; model.preview = undefined; model.changed(); }), button("Review native target", () => { void model.reviewTarget(); }, !model.canReview));
  details.append(node("p", "Select and attach a Session before reviewing a native target."));
}
function captureEvidence(details: HTMLElement, model: ComputerControls): void {
  const frame = model.latestFrame;
  if (frame) { details.append(node("p", `Latest private capture: ${frame.id} · ${new Date(frame.created_ms).toLocaleTimeString()} · ${frame.bytes} bytes`), button("View authorized capture", () => { void model.previewFrame(); }, model.frameBusy)); }
  if (model.imageURL) { const image = node("img"); image.className = "computer-capture"; image.alt = "Authorized private native screenshot"; image.src = model.imageURL; details.append(image); }
}
function targetReview(details: HTMLElement, model: ComputerControls): void {
  const preview = model.preview; if (!preview) return;
  details.append(node("p", preview.warning), node("pre", JSON.stringify(preview.target, null, 2)), node("p", `Review expires ${new Date(preview.expires_at).toLocaleTimeString()}. Grant lasts ${model.ttl} seconds.`));
  if (preview.unknown_operations.length) { details.append(node("pre", JSON.stringify(preview.unknown_operations, null, 2)), checkbox("I checked these unresolved operations on the host; they may already have happened", model.acknowledgeUnknown, (value) => { model.acknowledgeUnknown = value; model.changed(); })); }
  details.append(checkbox("I reviewed this exact target and authorize native capture/input", model.reviewed, (value) => { model.reviewed = value; model.changed(); }), button("Arm reviewed target", () => { void model.allow(); }, !model.canAllow));
}
