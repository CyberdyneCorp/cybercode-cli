import assert from "node:assert/strict";
import { test } from "node:test";
import { ComputerControls, type ComputerContext, type NativePreview, type NativeReport } from "../src/computer.js";
import type { Port, RequestOptions } from "../src/transport.js";
const report: NativeReport = { enabled: true, health: { supported: true, platform: "fixture", minimum_version: "fixture", screen_capture: true, accessibility: true, binary: "fixture" }, grant: { id: "grant", session_id: "other-session", target: { identity: "app:fixture" }, expires_at: 1_000_000 } };
const preview: NativePreview = { id: "review", session_id: "owner", expires_at: 1000, target: { identity: "app:fixture", app: "fixture", display: 1, pid: 42, window: 7, x: 0, y: 0, width: 100, height: 100, image_width: 100, image_height: 100 }, unknown_operations: [], warning: "fixture" };
function fixture(t: { after(callback: () => void): void }) {
 const calls: Array<{ path: string; options: RequestOptions }> = []; let context: ComputerContext | undefined = { session: "owner", directory: "/repo", actionable: true, signal: new AbortController().signal };
 let hook: ((path: string) => Promise<unknown>) | undefined;
 const api = { request: async <T>(path: string, options: RequestOptions = {}) => { calls.push({ path, options }); return (hook ? await hook(path) : { data: path.endsWith("/preview") ? preview : report }) as T; } } as Port;
 const model = new ComputerControls(api, () => context, () => {}, () => 100); t.after(() => { context = undefined; model.clear(); });
 return { model, calls, context: (value: ComputerContext | undefined) => { context = value; }, hook: (value: (path: string) => Promise<unknown>) => { hook = value; } };
}
test("global host banner exposes another Session's grant; reviews require explicit owner confirmation", async (t) => {
 const f = fixture(t); await f.model.refresh(); assert.equal(f.model.active?.session_id, "other-session"); f.model.selector = "fixture"; await f.model.reviewTarget(); assert.equal(f.model.canAllow, false); f.model.reviewed = true; assert.equal(f.model.canAllow, true); await f.model.allow();
 const allow = f.calls.find(call => call.path.endsWith("/allow")); assert.deepEqual(allow?.options.body, { preview_id: "review", confirm: true, ttl_seconds: 300, acknowledge_unknown: false }); assert.equal(allow?.options.directory, "/repo");
});
test("switches fence delayed review replies and unknown operations require acknowledgement", async (t) => {
 const f = fixture(t); let finish!: (value: unknown) => void; f.hook(path => path.endsWith("/preview") ? new Promise(resolve => { finish = resolve; }) : Promise.resolve({ data: report })); f.model.selector = "fixture"; const task = f.model.reviewTarget(); f.model.reset(); finish({ data: preview }); await task; assert.equal(f.model.preview, undefined);
 await f.model.refresh(); f.model.preview = { ...preview, unknown_operations: [{ operation_id: "uncertain", action: "type", started_ms: 1 }] }; f.model.reviewed = true; assert.equal(f.model.canAllow, false); f.model.acknowledgeUnknown = true; assert.equal(f.model.canAllow, true);
});
test("Stop stays available during blocked owner actions and uncertain responses never replay", async (t) => {
 const f = fixture(t); await f.model.refresh(); let finish!: (value: unknown) => void; f.hook(path => path.endsWith("/preview") ? new Promise(resolve => { finish = resolve; }) : Promise.resolve({ data: report })); f.model.selector = "fixture"; const task = f.model.reviewTarget(); assert.equal(f.model.busy, true); await f.model.stop(); assert.equal(f.calls.filter(call => call.path.endsWith("/stop")).length, 1); finish({ data: preview }); await task; assert.equal(f.model.preview, undefined);
 f.hook(() => Promise.reject(new Error("fixture disconnect"))); await f.model.stop(); assert.equal(f.model.uncertain, true); assert.match(f.model.notice, /could not be confirmed/);
});
