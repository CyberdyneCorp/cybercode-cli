/**
 * Activation (`editor-integration` → VS Code extension): the chat view, the review
 * content provider and the commands. Nothing here stores a credential: no setting accepts
 * one, and `SecretStorage`, `globalState` and `workspaceState` are never given the token
 * (`workspaceState` holds only the bound Session ID).
 */
import * as vscode from "vscode";
import { ChatController, type Hooks } from "./panel.js";
import { ReviewManager, SCHEME } from "./review.js";
import { mention, selectedLines } from "./mention.js";
import { stateReads } from "./server.js";
import type { HostMessage } from "../shared/protocol.js";

let controller: ChatController | undefined;

/** What integration tests inspect; exported only when `CYBER_VSCODE_TEST=1`. */
export interface TestApi {
  chat: ChatController;
  reviews: ReviewManager;
  posted: HostMessage[];
  logged: string[];
  stateReads: string[];
}

function sessionTerminal(chat: ChatController, resume: boolean): void {
  const executable = chat.executable;
  if (!vscode.workspace.isTrusted) {
    void vscode.window.showWarningMessage("Cyber Code terminals are unavailable in Restricted Mode.");
    return;
  }
  if (!executable) {
    void vscode.window.showErrorMessage("Cyber Code CLI not found.");
    return;
  }
  const session = chat.state.sessionId;
  if (resume && !session) {
    void vscode.window.showWarningMessage("Bind the Cyber Code panel to a Session first.");
    return;
  }
  const terminal = vscode.window.createTerminal({ name: "Cyber Code", shellPath: executable, shellArgs: resume ? ["--resume", session] : [], cwd: chat.location });
  terminal.show();
}

function insertSelectionMention(chat: ChatController): void {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.uri.scheme !== "file") return;
  const lines = selectedLines(editor.selection.start, editor.selection.end) ?? { start: editor.selection.active.line + 1, end: editor.selection.active.line + 1 };
  chat.insertMention(mention(chat.location, editor.document.uri.fsPath, lines));
  void vscode.commands.executeCommand("cyber.chat.focus");
}

async function sessionChanges(chat: ChatController, reviews: ReviewManager): Promise<void> {
  const client = chat.client;
  if (!client || !chat.state.sessionId) {
    void vscode.window.showWarningMessage("Bind the Cyber Code panel to a Session first.");
    return;
  }
  try {
    const files = (await client.request("v1.session.diff", { path: { sessionID: chat.state.sessionId } })) as { file: string; patch: string }[];
    await reviews.sessionChanges(files, chat.location);
  } catch (error) {
    await chat.failed(error);
  }
}

export function activate(context: vscode.ExtensionContext): TestApi | undefined {
  const testing = process.env.CYBER_VSCODE_TEST === "1";
  const output = vscode.window.createOutputChannel("Cyber Code");
  const logged: string[] = [];
  const log = (line: string) => {
    if (testing) logged.push(line);
    output.appendLine(line);
  };
  const hooks: Hooks = testing ? { posted: [] } : {};
  const chat = new ChatController(context, log, hooks);
  controller = chat;
  const reviews = new ReviewManager(chat);
  chat.openReview = (request, location) => void reviews.open(request, location);
  chat.onAnswered = (requestId, elsewhere) => void reviews.close(requestId, elsewhere);
  const command = (name: string, run: () => unknown) => vscode.commands.registerCommand(name, run);
  context.subscriptions.push(
    output,
    vscode.window.registerWebviewViewProvider("cyber.chat", chat, { webviewOptions: { retainContextWhenHidden: false } }),
    vscode.workspace.registerTextDocumentContentProvider(SCHEME, reviews),
    vscode.workspace.onDidGrantWorkspaceTrust(() => void chat.connect()),
    vscode.window.onDidChangeActiveTextEditor(() => chat.emit()),
    vscode.window.onDidChangeTextEditorSelection(() => chat.emit()),
    vscode.workspace.onDidSaveTextDocument(() => chat.emit()),
    command("cyber.insertSelectionMention", () => insertSelectionMention(chat)),
    command("cyber.reviewSessionChanges", () => sessionChanges(chat, reviews)),
    command("cyber.openSessionInTerminal", () => sessionTerminal(chat, true)),
    command("cyber.newTerminalSession", () => sessionTerminal(chat, false)),
    command("cyber.createIdeToken", () => chat.createTokenInTerminal()),
    command("cyber.review.accept", () => reviews.decide("accept")),
    command("cyber.review.acceptAlways", () => reviews.decide("acceptAlways")),
    command("cyber.review.reject", () => reviews.decide("reject")),
    command("cyber.review.rejectWithFeedback", () => reviews.decide("rejectWithFeedback")),
  );
  return testing ? { chat, reviews, posted: hooks.posted ?? [], logged, stateReads } : undefined;
}

export async function deactivate(): Promise<void> {
  await controller?.dispose();
  controller = undefined;
}
