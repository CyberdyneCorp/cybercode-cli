/** `editor-integration` → Extension workspace trust, "Restricted Mode". */
import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import * as vscode from "vscode";
import type { FixtureInfo } from "../fixture";
import { eventually, extensionApi } from "./chat.test";

const fixture = JSON.parse(process.env.CYBER_TEST_FIXTURE ?? "{}") as FixtureInfo;

export function restrictedSuite(): void {
  test("Restricted Mode: nothing starts, the composer is disabled, trust is offered", async () => {
    assert.equal(vscode.workspace.isTrusted, false);
    const registered = readFileSync(join(fixture.home, "state/server.json"), "utf8");
    const api = await extensionApi();
    await vscode.commands.executeCommand("cyber.chat.focus");
    await eventually(() => api.chat.state.phase, (phase) => phase === "restricted", "Restricted Mode");
    assert.deepEqual(api.chat.state.actions, ["manageTrust"]);
    await api.chat.receive({ type: "prompt", text: "should not send", includeContext: false });
    await api.chat.receive({ type: "newSession" });
    assert.equal(api.chat.client, undefined, "no connection, so no request");
    assert.ok(!api.logged.some((line) => line.startsWith("POST")));
    await vscode.commands.executeCommand("cyber.openSessionInTerminal");
    assert.equal(vscode.window.terminals.length, 0);
    assert.equal(readFileSync(join(fixture.home, "state/server.json"), "utf8"), registered);
  });
}
