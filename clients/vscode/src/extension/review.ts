/**
 * Native diff review (`editor-integration` → Pending edit diff review): a pending edit's
 * hunks are applied to a snapshot of the on-disk file taken when the review opens, and both
 * sides are served read-only under `cyber-review:` URIs that name the request but hold no
 * content. Accept, Accept always, Reject and Reject with feedback answer the whole
 * request; the extension never writes the real file. Completed Session changes open
 * read-only the same way.
 */
import * as vscode from "vscode";
import { isAbsolute, join, basename } from "node:path";
import type { PendingRequest } from "./approvals.js";
import type { ChatController } from "./panel.js";
import { applyPatch, parseUnifiedDiff, reversePatch, type FilePatch } from "./diff.js";

export const SCHEME = "cyber-review";
export const CHANGED = "The file changed since Cyber proposed this edit";

interface Review {
  requestId: string;
  sessionId: string;
  alwaysPatterns: string[];
  uris: vscode.Uri[];
}

export class ReviewManager implements vscode.TextDocumentContentProvider {
  readonly #documents = new Map<string, string>();
  readonly #reviews = new Map<string, Review>();
  readonly #changed = new vscode.EventEmitter<vscode.Uri>();
  readonly onDidChange = this.#changed.event;
  #active: string | undefined;
  #counter = 0;

  constructor(private readonly chat: ChatController) {}

  provideTextDocumentContent(uri: vscode.Uri): string {
    return this.#documents.get(uri.toString()) ?? "";
  }

  get active(): string | undefined {
    return this.#active;
  }

  private document(requestId: string, side: string, path: string, content: string): vscode.Uri {
    const uri = vscode.Uri.from({ scheme: SCHEME, path: `/${requestId}/${++this.#counter}-${side}/${basename(path) || "file"}` });
    this.#documents.set(uri.toString(), content);
    return uri;
  }

  private async disk(location: string, path: string): Promise<string> {
    const uri = vscode.Uri.file(isAbsolute(path) ? path : join(location, path));
    try {
      return new TextDecoder().decode(await vscode.workspace.fs.readFile(uri));
    } catch {
      return "";
    }
  }

  /** Open the review of a pending edit request in the diff or multi-file diff editor. */
  async open(request: PendingRequest, location: string): Promise<void> {
    const diff = (request.metadata as { diff?: unknown } | undefined)?.diff;
    if (typeof diff !== "string") return;
    const patches = parseUnifiedDiff(diff);
    const pairs: [vscode.Uri, vscode.Uri, vscode.Uri][] = [];
    for (const patch of patches) {
      const path = patch.newPath || patch.oldPath;
      const base = await this.disk(location, path);
      const proposed = applyPatch(base, patch);
      if (proposed === undefined) return this.openRaw(request, diff);
      const target = vscode.Uri.file(isAbsolute(path) ? path : join(location, path));
      pairs.push([target, this.document(request.id, "base", path, base), this.document(request.id, "proposed", path, proposed)]);
    }
    this.track(request, pairs.flatMap(([, base, proposed]) => [base, proposed]));
    await this.show(pairs, "Cyber proposal");
  }

  private async show(pairs: [vscode.Uri, vscode.Uri, vscode.Uri][], title: string): Promise<void> {
    if (pairs.length === 1) {
      const [target, base, proposed] = pairs[0]!;
      await vscode.commands.executeCommand("vscode.diff", base, proposed, `${vscode.workspace.asRelativePath(target)} (${title})`, { preview: false });
    } else if (pairs.length > 1) {
      await vscode.commands.executeCommand("vscode.changes", title, pairs);
    }
  }

  /** The hunks do not apply: show the raw diff read-only, still with the reply actions. */
  private async openRaw(request: PendingRequest, diff: string): Promise<void> {
    const uri = this.document(request.id, "raw", "proposal.diff", `# ${CHANGED}\n${diff}`);
    this.track(request, [uri]);
    const document = await vscode.workspace.openTextDocument(uri);
    await vscode.languages.setTextDocumentLanguage(document, "diff");
    await vscode.window.showTextDocument(document, { preview: false });
    void vscode.window.showWarningMessage(CHANGED);
  }

  private track(request: PendingRequest, uris: vscode.Uri[]): void {
    this.#reviews.set(request.id, { requestId: request.id, sessionId: request.session_id, alwaysPatterns: request.always_patterns ?? [], uris });
    this.activate(request.id);
  }

  private activate(requestId: string | undefined): void {
    this.#active = requestId;
    const review = requestId ? this.#reviews.get(requestId) : undefined;
    void vscode.commands.executeCommand("setContext", "cyber.reviewPending", review !== undefined);
    void vscode.commands.executeCommand("setContext", "cyber.reviewAlways", (review?.alwaysPatterns.length ?? 0) > 0);
  }

  /** The reply actions of the editor title bar, for the active review. */
  async decide(choice: "accept" | "acceptAlways" | "reject" | "rejectWithFeedback"): Promise<void> {
    const review = this.#active ? this.#reviews.get(this.#active) : undefined;
    if (!review) return;
    if (choice === "acceptAlways") {
      const confirm = await vscode.window.showWarningMessage(`Accept always saves these patterns: ${review.alwaysPatterns.join(", ")}`, { modal: true }, "Accept always");
      if (confirm !== "Accept always") return;
      await this.chat.reply(review.sessionId, review.requestId, { reply: "always" });
      return;
    }
    if (choice === "rejectWithFeedback") {
      const message = await vscode.window.showInputBox({ prompt: "Feedback for the model (rejecting declines the Session's other pending requests)" });
      if (message === undefined) return;
      await this.chat.reply(review.sessionId, review.requestId, { reply: "reject", message });
      return;
    }
    await this.chat.reply(review.sessionId, review.requestId, { reply: choice === "accept" ? "once" : "reject" });
  }

  /** The request was answered (here or elsewhere): close its editors and forget it. */
  async close(requestId: string, elsewhere: boolean): Promise<void> {
    const review = this.#reviews.get(requestId);
    if (!review) return;
    this.#reviews.delete(requestId);
    const names = new Set(review.uris.map((uri) => uri.toString()));
    const tabs = vscode.window.tabGroups.all.flatMap((group) => group.tabs).filter((tab) => tabUris(tab).some((uri) => names.has(uri)));
    await vscode.window.tabGroups.close(tabs);
    for (const name of names) this.#documents.delete(name);
    if (this.#active === requestId) this.activate([...this.#reviews.keys()].at(-1));
    void vscode.window.showInformationMessage(elsewhere ? "Cyber Code: this request was answered elsewhere." : "Cyber Code: reply sent.");
  }

  /** `Cyber: Review Session Changes`: the Session's diff, read-only. */
  async sessionChanges(files: { file: string; patch: string }[], location: string): Promise<void> {
    const pairs: [vscode.Uri, vscode.Uri, vscode.Uri][] = [];
    for (const file of files) {
      const current = await this.disk(location, file.file);
      const patch: FilePatch | undefined = parseUnifiedDiff(file.patch)[0];
      const before = patch ? applyPatch(current, reversePatch(patch)) : undefined;
      const target = vscode.Uri.file(isAbsolute(file.file) ? file.file : join(location, file.file));
      pairs.push([target, this.document("changes", "before", file.file, before ?? file.patch), this.document("changes", "after", file.file, current)]);
    }
    if (pairs.length === 0) {
      void vscode.window.showInformationMessage("Cyber Code: this Session has no changes.");
      return;
    }
    await vscode.commands.executeCommand("vscode.changes", "Cyber Session changes", pairs);
  }
}

function tabUris(tab: vscode.Tab): string[] {
  const input = tab.input;
  if (input instanceof vscode.TabInputTextDiff) return [input.original.toString(), input.modified.toString()];
  if (input instanceof vscode.TabInputText) return [input.uri.toString()];
  return [];
}
