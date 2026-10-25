//! Native owner controls. No model/remote peer can create an owner review.
import type { Port } from "./transport.js";
export interface ComputerContext { session: string; directory: string; actionable: boolean; signal: AbortSignal }
export interface NativeGrant { id: string; session_id: string; target: { identity: string }; expires_at: number }
interface NativeHealth { supported: boolean; platform: string; minimum_version: string; screen_capture: boolean; accessibility: boolean; binary: string }
export interface NativeReport { enabled: boolean; health: NativeHealth; grant: NativeGrant | null }
interface UnknownOperation { operation_id: string; action: string; started_ms: number }
export interface NativeFrame {id:string;session_id:string;created_ms:number;sha256:string;bytes:number;target:NativePreview["target"]}
export interface NativePreview { id: string; session_id: string; expires_at: number; target: { identity: string; app: string | null; display: number; pid: number; window: number; x: number; y: number; width: number; height: number; image_width: number; image_height: number }; unknown_operations: UnknownOperation[]; warning: string }

/** State owns review freshness and uncertainty; the view only renders and invokes actions. */
export class ComputerControls {
  report: NativeReport | undefined;
  preview: NativePreview | undefined;
  latestFrame: NativeFrame | undefined; imageURL: string | undefined; frameBusy = false;
  notice = ""; busy = false; uncertain = false; reviewed = false; acknowledgeUnknown = false;
  selector = ""; display = false; ttl = 300;
  #generation = 0; #refreshing = false; #timer: ReturnType<typeof setTimeout> | undefined;
  constructor(readonly api: Port, readonly context: () => ComputerContext | undefined, readonly changed: () => void, readonly now = Date.now) {}
  reset(): void { this.clearImage(); this.latestFrame = undefined; this.frameBusy = false; this.#generation++; clearTimeout(this.#timer); this.#refreshing = false; this.busy = false; this.preview = undefined; this.reviewed = false; this.acknowledgeUnknown = false; this.uncertain = !!this.report?.grant; this.changed(); }
  clear(): void { this.reset(); this.report = undefined; this.uncertain = false; this.notice = ""; }
  get active(): NativeGrant | undefined { const grant = this.report?.grant; return grant && (grant.expires_at > this.now() || this.uncertain) ? grant : undefined; }
  get canAllow(): boolean { const context = this.context(); return !!this.report?.enabled && !!this.report.health.supported && !!context?.actionable && !!this.preview && this.preview.session_id === context.session && this.preview.expires_at > this.now() && this.reviewed && (!this.preview.unknown_operations.length || this.acknowledgeUnknown) && !this.busy && !this.uncertain; }
  async refresh(): Promise<void> {
    const context = this.context(); if (!context || this.#refreshing) return;
    const generation = this.#generation; this.#refreshing = true;
    try { const value = await this.api.request<{ data: NativeReport }>("/tools/computer/health", { directory: context.directory, signal: context.signal }); if (generation === this.#generation) { this.report = value.data; this.uncertain = false; await this.refreshFrame(context,generation); } }
    catch { if (generation === this.#generation) this.uncertain = !!this.report?.grant; }
    finally { if (generation === this.#generation) { this.#refreshing = false; this.changed(); clearTimeout(this.#timer); this.#timer = setTimeout(() => { void this.refresh(); }, 2000); } }
  }
  private async refreshFrame(context:ComputerContext,generation:number):Promise<void> {
    try {
      const status=await this.api.request<{data:{latest_frame?:NativeFrame|null}}>(`/sessions/${encodeURIComponent(context.session)}/computer`,{directory:context.directory,signal:context.signal});
      if(generation!==this.#generation)return;
      const frame=status.data.latest_frame??undefined;
      if(frame?.id!==this.latestFrame?.id)this.clearImage();this.latestFrame=frame;
    } catch {if(generation===this.#generation){this.clearImage();this.latestFrame=undefined;}}
  }
  private clearImage():void {if(this.imageURL)URL.revokeObjectURL(this.imageURL);this.imageURL=undefined;}
  async previewFrame():Promise<void> {
    const context=this.context();const frame=this.latestFrame;if(!context?.actionable||!frame||frame.session_id!==context.session||this.frameBusy)return;
    const generation=this.#generation;this.frameBusy=true;this.changed();
    try {
      const bytes=await this.api.bytes(`/sessions/${encodeURIComponent(context.session)}/computer/frames/${encodeURIComponent(frame.id)}`,{directory:context.directory,signal:context.signal});
      if(generation!==this.#generation||this.latestFrame?.id!==frame.id)return;
      this.clearImage();this.imageURL=URL.createObjectURL(new Blob([new Uint8Array(bytes).buffer],{type:"image/png"}));
    } catch {if(generation===this.#generation)this.notice="Private capture is unavailable or expired. Request a fresh authorized capture.";}
    finally {if(generation===this.#generation){this.frameBusy=false;this.changed();}}
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
