import { ApiFault, operationKey, type Port } from "./transport.js";
import type { IntegrationBatch as ApiIntegrationBatch } from "../../sdk/typescript/src/generated/types.js";
export type IntegrationBatch = Pick<
  ApiIntegrationBatch,
  | "id"
  | "revision"
  | "status"
  | "run_id"
  | "manifest"
  | "report"
  | "publication"
>;
interface Context {
  directory: string;
  session?: string | undefined;
  enabled: boolean;
}
const initialManifest =
  '{"contributions":[{"label":"api","commit":"FULL_LOCAL_COMMIT_HASH"}],"checks":[{"argv":["cargo","test"],"timeout_seconds":300}],"reviewer_model":"verify"}';
interface PendingIntegration {
  suffix: string;
  body: unknown;
  key: string;
  directory: string;
}
/** Authority and uncertainty stay attached to one authenticated Location. */
export class Integrations {
  rows: IntegrationBatch[] = [];
  selected: IntegrationBatch | undefined;
  busy = false;
  reviewed = false;
  notice = "";
  manifest = initialManifest;
  publication = {
    remote: "origin",
    repository: "",
    title: "",
    body: "",
    acknowledge_review_unavailable: false,
  };
  #epoch = 0;
  #pending: PendingIntegration | undefined;
  get uncertain(): boolean {
    return !!this.#pending && !this.busy;
  }
  constructor(
    readonly api: Port,
    readonly context: () => Context,
    readonly changed: () => void,
  ) {}
  reset(): void {
    this.#epoch++;
    this.#pending = undefined;
    this.manifest = initialManifest;
    this.rows = [];
    this.selected = undefined;
    this.reviewed = false;
    this.notice = "";
    this.busy = false;
    this.publication = {
      remote: "origin",
      repository: "",
      title: "",
      body: "",
      acknowledge_review_unavailable: false,
    };
  }
  async load(id?: string): Promise<boolean> {
    const epoch = this.#epoch,
      context = this.context();
    if (!context.enabled) return false;
    try {
      const path = id
        ? `/workflows/integrations/${encodeURIComponent(id)}/report`
        : "/workflows/integrations";
      const result = await this.api.request<{
        data: IntegrationBatch | IntegrationBatch[];
      }>(path, { directory: context.directory });
      if (
        epoch !== this.#epoch ||
        context.directory !== this.context().directory
      )
        return false;
      if (Array.isArray(result.data)) this.rows = result.data;
      else {
        this.selected = result.data;
        this.reviewed = false;
      }
      this.changed();
      return true;
    } catch (error) {
      if (epoch === this.#epoch) this.notice = String(error);
      this.changed();
      return false;
    }
  }
  async submit(): Promise<void> {
    const session = this.context().session;
    if (!session) return;
    try {
      await this.action("", {
        session_id: session,
        manifest: JSON.parse(this.manifest),
      });
    } catch (error) {
      this.notice = String(error);
      this.changed();
    }
  }
  async execute(operation: "run" | "cancel" | "publish"): Promise<void> {
    const batch = this.selected;
    if (!batch || (operation === "publish" && !this.reviewed)) return;
    const body =
      operation === "publish"
        ? {
            ...this.publication,
            operation_id: operationKey(),
            expected_revision: batch.revision,
          }
        : { expected_revision: batch.revision };
    await this.action(`/${encodeURIComponent(batch.id)}/${operation}`, body);
  }
  get pendingTarget(): string {
    const pending = this.#pending;
    if (!pending) return "";
    const body = pending.body as Record<string, unknown>;
    return pending.suffix
      ? `${pending.suffix.split("/")[1]} · ${pending.suffix.split("/")[2]} · revision ${body.expected_revision}`
      : `Submission for owner Session ${body.session_id}`;
  }
  async discardRetry(): Promise<void> {
    const pending = this.#pending;
    if (!pending || this.busy) return;
    if ((await this.load()) && this.#pending === pending) {
      this.#pending = undefined;
      this.notice =
        "Queue refreshed and local retry cleared. This does not cancel an original action that is still running.";
      this.changed();
    }
  }
  async retry(): Promise<void> {
    if (this.#pending) await this.send(this.#pending);
  }
  private async action(suffix: string, body: unknown): Promise<void> {
    if (this.#pending) return;
    await this.send({
      suffix,
      body: structuredClone(body),
      key: operationKey(),
      directory: this.context().directory,
    });
  }
  private async send(command: PendingIntegration): Promise<void> {
    const context = this.context(),
      epoch = this.#epoch;
    if (
      !context.enabled ||
      this.busy ||
      command.directory !== context.directory
    )
      return;
    this.#pending = command;
    this.busy = true;
    this.notice = "Awaiting server acknowledgement…";
    this.changed();
    try {
      const result = await this.api.request<{ data: IntegrationBatch }>(
        `/workflows/integrations${command.suffix}`,
        {
          method: "POST",
          directory: command.directory,
          key: command.key,
          body: command.body,
        },
      );
      if (epoch !== this.#epoch) return;
      this.#pending = undefined;
      this.selected = result.data;
      this.reviewed = false;
      this.notice = "Server acknowledged. Refresh for current host evidence.";
      await this.load();
    } catch (error) {
      if (epoch !== this.#epoch) return;
      if (
        error instanceof ApiFault &&
        ["refused", "missing"].includes(error.kind)
      )
        this.#pending = undefined;
      this.notice = `${String(error)} Refresh current evidence. An uncertain retry keeps the original action, revision and request key.`;
    } finally {
      if (epoch === this.#epoch) {
        this.busy = false;
        this.changed();
      }
    }
  }
}
