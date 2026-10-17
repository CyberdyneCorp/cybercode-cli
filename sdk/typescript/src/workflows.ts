import type { StreamOptions } from "./generated/operations.js";
import type {
  EventEnvelope,
  Page_WorkflowRunSummary,
  ResumeBody,
  RunDetail,
  StartBody,
  WorkflowRun,
  WorkflowRunProgress,
  WorkflowRunStatus,
  WorkflowRunSummary,
} from "./generated/types.js";
import { sleep } from "./sleep.js";

/** What `client.workflows` needs from its client (`client-sdk` → Workflow runs). */
export interface WorkflowCaller {
  readonly session: {
    create(body: { title?: string | null }): Promise<{ data: { id: string } }>;
  };
  readonly workflow: {
    run(body: StartBody): Promise<WorkflowRun>;
    get(id: string): Promise<RunDetail>;
    list(query?: WorkflowListQuery, options?: { signal?: AbortSignal }): Promise<Page_WorkflowRunSummary>;
    pause(id: string): Promise<WorkflowRun>;
    resume(id: string, body?: ResumeBody): Promise<WorkflowRun>;
    stop(id: string): Promise<WorkflowRun>;
  };
  readonly events: {
    subscribe(options?: StreamOptions & { scope?: "all" }): AsyncIterableIterator<EventEnvelope>;
  };
}

/**
 * Start a run from a saved Workflow (`name`) or an inline module (`script`). Without
 * `session_id`, a new owner Session is created in the client's Location.
 */
export type WorkflowRunInput = Omit<StartBody, "session_id"> & { session_id?: string };

/** The query `GET /api/v1/workflows/runs` takes. */
type WorkflowListQuery = { status?: string; session_id?: string; directory?: string; limit?: number; cursor?: string };

/** Which runs `client.workflows.list()` returns (`client-sdk` → Workflow helpers). */
export interface WorkflowListFilter {
  /** Only runs with one of these statuses. */
  status?: WorkflowRunStatus | readonly WorkflowRunStatus[];
  /** Only runs owned by this Session. */
  session_id?: string;
  /** A Location, or `"all"` for every Location (default: the client's Location). */
  directory?: string;
  /** Runs per page, 1–200 (default 50); further pages are read as the iteration reaches them. */
  limit?: number;
  signal?: AbortSignal;
}

export interface ResultOptions {
  /** How often `status()` is polled (default 500 ms). */
  pollMs?: number;
  signal?: AbortSignal;
}

/** Statuses after which the run does not continue without a resume. */
const ENDED: readonly WorkflowRunStatus[] = ["completed", "failed", "budget_exceeded", "stopped", "paused"];

/** A run that ended without completing: `run` holds its status, reason and diagnostic. */
export class WorkflowRunError extends Error {
  override readonly name = "WorkflowRunError";
  readonly run: RunDetail;

  constructor(run: RunDetail) {
    const detail = run.error ? `${run.error.kind}: ${run.error.message}` : (run.reason ?? run.status);
    super(`Workflow Run ${run.id} ${run.status}: ${detail}`);
    this.run = run;
  }
}

/** A started Workflow Run. */
export interface WorkflowRunHandle {
  readonly id: string;
  /** The run as it was started, or as `list()` or `get()` read it. */
  readonly run: WorkflowRun;
  /**
   * The run's summary (agent counts, tokens, cost, active time) when the handle came from
   * `list()` or `get()`; absent on a handle `run()` returned.
   */
  readonly summary?: WorkflowRunSummary;
  /** The run now, with its agents. */
  status(): Promise<RunDetail>;
  /**
   * Live events of this run from the Location's event stream: its durable `workflow.*`
   * events and its `workflow.run.progress` events.
   */
  events(options?: StreamOptions): AsyncIterableIterator<EventEnvelope>;
  /** In-flight agents finish; no new agent starts. */
  pause(): Promise<WorkflowRun>;
  /** Relaunch through replay; `revision: true` accepts a changed script or configuration. */
  resume(body?: ResumeBody): Promise<WorkflowRun>;
  /** Stop the run and cancel its in-flight agents. */
  cancel(): Promise<WorkflowRun>;
  /**
   * The script's return value once the run completes. A run that fails, exceeds its
   * budget, stops or pauses rejects with a `WorkflowRunError`. A return value larger than
   * 1 MiB resolves to `{ result_file, result_preview }`.
   */
  result(options?: ResultOptions): Promise<unknown>;
}

export async function startWorkflowRun(client: WorkflowCaller, input: WorkflowRunInput): Promise<WorkflowRunHandle> {
  const session_id = input.session_id ?? (await client.session.create({ title: `Workflow: ${input.name ?? "inline"}` })).data.id;
  const run = await client.workflow.run({ ...input, session_id });
  return handle(client, run);
}

/** The handle of an existing run, carrying its summary. */
export async function getWorkflowRun(client: WorkflowCaller, id: string): Promise<WorkflowRunHandle> {
  const detail = await client.workflow.get(id);
  return handle(client, detail, detail);
}

/** Handles of the listed runs, active runs first, reading each further page when reached. */
export async function* listWorkflowRuns(client: WorkflowCaller, filter: WorkflowListFilter = {}): AsyncIterableIterator<WorkflowRunHandle> {
  const { signal, status, ...rest } = filter;
  const query: WorkflowListQuery = { ...rest, ...(status !== undefined && { status: typeof status === "string" ? status : status.join(",") }) };
  let cursor: string | undefined;
  do {
    const page = await client.workflow.list({ ...query, ...(cursor !== undefined && { cursor }) }, { signal });
    for (const summary of page.data) yield handle(client, summary, summary);
    cursor = page.cursor.next ?? undefined;
  } while (cursor !== undefined);
}

function handle(client: WorkflowCaller, run: WorkflowRun, summary?: WorkflowRunSummary): WorkflowRunHandle {
  const id = run.id;
  const status = () => client.workflow.get(id);
  return {
    id,
    run,
    ...(summary && { summary }),
    status,
    events: (options = {}) => runEvents(client.events.subscribe(options), id),
    pause: () => client.workflow.pause(id),
    resume: (body) => client.workflow.resume(id, body),
    cancel: () => client.workflow.stop(id),
    result: async ({ pollMs = 500, signal } = {}) => {
      for (;;) {
        const current = await status();
        if (ENDED.includes(current.status)) return outcome(current);
        if (signal?.aborted) throw signal.reason ?? new Error("aborted");
        await sleep(pollMs, signal);
      }
    },
  };
}

function outcome(run: RunDetail): unknown {
  if (run.status !== "completed") throw new WorkflowRunError(run);
  if (run.result_file) return { result_file: run.result_file, result_preview: run.result_preview };
  return run.result;
}

async function* runEvents(events: AsyncIterableIterator<EventEnvelope>, id: string): AsyncIterableIterator<EventEnvelope> {
  for await (const event of events) {
    if (ofRun(event, id)) yield event;
  }
}

/** A durable `workflow.*` event of run `id`, or its `workflow.run.progress` event. */
function ofRun(event: EventEnvelope, id: string): boolean {
  if (!event.type.startsWith("workflow.")) return false;
  if (event.type === PROGRESS) return (event.data as Partial<WorkflowRunProgress> | null)?.run_id === id;
  return event.durable?.aggregateID === id;
}

const PROGRESS = "workflow.run.progress";
