import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

const manifest = JSON.parse(readFileSync(join(__dirname, "../../package.json"), "utf8"));
const settings: Record<string, { scope?: string; type: string }> = manifest.contributes.configuration.properties;

test("no setting accepts a password or a token", () => {
  for (const name of Object.keys(settings)) assert.ok(!/password|token|secret|credential/i.test(name), name);
});

test("executablePath and autoStartServer are machine-scoped; autoStartServer is restricted", () => {
  assert.equal(settings["cyber.executablePath"]?.scope, "machine");
  assert.equal(settings["cyber.autoStartServer"]?.scope, "machine");
  assert.deepEqual(manifest.capabilities.untrustedWorkspaces.restrictedConfigurations, ["cyber.autoStartServer"]);
  assert.equal(manifest.capabilities.untrustedWorkspaces.supported, "limited");
});

test("identity, engine and local-only extension kind", () => {
  assert.equal(manifest.name, "cyber");
  assert.equal(manifest.publisher, "cyber-code");
  assert.equal(manifest.engines.vscode, "^1.95.0");
  assert.deepEqual(manifest.extensionKind, ["workspace"]);
  assert.equal(manifest.browser, undefined);
  assert.equal(manifest.contributes.views.cyber[0].id, "cyber.chat");
});

test("the release version check: the extension version equals the cyber version", () => {
  const cargo = readFileSync(join(__dirname, "../../../../Cargo.toml"), "utf8");
  const version = /^version = "([^"]+)"/m.exec(cargo)?.[1];
  assert.equal(manifest.version, version);
});
