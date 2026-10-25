//! Native owner controls. No model/remote peer can create an owner review.
import type { Port } from "./transport.js";
export interface ComputerContext { session: string; directory: string; actionable: boolean; signal: AbortSignal }
export interface NativeGrant { id: string; session_id: string; target: { identity: string }; expires_at: number }
interface NativeHealth { supported: boolean; platform: string; minimum_version: string; screen_capture: boolean; accessibility: boolean; binary: string }
export interface NativeReport { enabled: boolean; health: NativeHealth; grant: NativeGrant | null }
interface UnknownOperation { operation_id: string; action: string; started_ms: number }
export interface NativePreview { id: string; session_id: string; expires_at: number; target: { identity: string; app: string | null; display: number; pid: number; window: number; x: number; y: number; width: number; height: number; image_width: number; image_height: number }; unknown_operations: UnknownOperation[]; warning: string }

/** State owns review freshness and uncertainty; the view only renders and invokes actions. */
export class ComputerControls {
  report: NativeReport | undefined;
  preview: NativePreview | undefined;
  notice = ""; busy = false; uncertain = false; reviewed = false; acknowledgeUnknown = false;
  selector = ""; display = false; ttl = 300;
  #generation = 0; #refreshing = false; #timer: ReturnType<typeof setTimeout> | undefined;
  constructor(readonly api: Port, readonly context: () => ComputerContext | undefined, readonly changed: () => void, readonly now = Date.now) {}
  reset(): void { this.#generation++; clearTimeout(this.#timer); this.#refreshing = false; this.busy = false; this.preview = undefined; this.reviewed = false; this.acknowledgeUnknown = false; this.uncertain = !!this.report?.grant; this.changed(); }
  clear(): void { this.reset(); this.report = undefined; this.uncertain = false; this.notice = ""; }
  get active(): NativeGrant | undefined { const grant = this.report?.grant; return grant && (grant.expires_at > this.now() || this.uncertain) ? grant : undefined; }
  get canAllow(): boolean { const context = this.context(); return !!this.report?.enabled && !!this.report.health.supported && !!context?.actionable && !!this.preview && this.preview.session_id === context.session && this.preview.expires_at > this.now() && this.reviewed && (!this.preview.unknown_operations.length || this.acknowledgeUnknown) && !this.busy && !this.uncertain; }
  async refresh(): Promise<void> {
    const context = this.context(); if (!context || this.#refreshing) return;
    const generation = this.#generation; this.#refreshing = true;
    try { const value = await this.api.request<{ data: NativeReport }>("/tools/computer/health", { directory: context.directory, signal: context.signal }); if (generation === this.#generation) { this.report = value.data; this.uncertain = false; } }
    catch { if (generation === this.#generation) this.uncertain = !!this.report?.grant; }
    finally { if (generation === this.#generation) { this.#refreshing = false; this.changed(); clearTimeout(this.#timer); this.#timer = setTimeout(() => { void this.refresh(); }, 2000); } }
  }
  async setup(): Promise<void> { await this.mutate("/tools/computer/setup", { confirm: true }); }
  async reviewTarget(): Promise<void> {
    const context = this.context(); if (!context?.actionable || this.busy) return;
    const selector = this.selector.trim(); const display = this.display ? Number(selector) : null;
    if (!selector || (this.display && (!Number.isInteger(display) || display! < 0 || display! > 0xffffffff))) { this.notice = "Enter an application bundle ID or a native display ID (0 selects the main display)."; this.changed(); return; }
    this.preview = undefined; this.reviewed = false; this.acknowledgeUnknown = false;
    await this.mutate(`/sessions/${encodeURIComponent(context.session)}/computer/preview`, { app: this.display ? null : selector, display }, true);
  }
  async allow(): Promise<void> {
    const context = this.context(); const preview = this.preview; if (!context || !preview || !this.canAllow) return;
    await this.mutate(`/sessions/${encodeURIComponent(context.session)}/computer/allow`, { preview_id: preview.id, confirm: true, ttl_seconds: this.ttl, acknowledge_unknown: this.acknowledgeUnknown });
    this.preview = undefined; this.reviewed = false;
  }
  /** Stop is independent of ordinary in-flight controls and is safe to repeat. */
  async stop(): Promise<void> {
    const context = this.context(); if (!context) return;
    this.#generation++; this.#refreshing = false; this.busy = false; clearTimeout(this.#timer);
    this.preview = undefined; this.reviewed = false;
    try { await this.api.request("/tools/computer/stop", { method: "POST", directory: context.directory }); this.report = this.report ? { ...this.report, grant: null } : undefined; this.uncertain = false; this.notice = "Native computer control stopped."; }
    catch { this.uncertain = true; this.notice = "Stop could not be confirmed. Use cyber computer stop on the host; do not assume input stopped."; }
    this.changed();
    void this.refresh();
  }
  private async mutate(path: string, body: unknown, preview = false): Promise<void> {
    const context = this.context(); if (!context?.actionable || this.busy) return;
    const generation = this.#generation; this.busy = true; this.notice = ""; this.changed();
    try { const value = await this.api.request<{ data: NativePreview }>(path, { method: "POST", body, directory: context.directory, signal: context.signal }); if (generation !== this.#generation) return; if (preview) this.preview = value.data; this.notice = preview ? "Review this exact native target; this review expires in 30 seconds." : "Native owner action completed."; }
    catch { if (generation === this.#generation) { this.preview = undefined; this.uncertain = true; this.notice = "Native action was not confirmed. Refresh status and review the target again; input is never automatically replayed."; } }
    finally { if (generation === this.#generation) { this.busy = false; this.changed(); await this.refresh(); } }
  }
}
