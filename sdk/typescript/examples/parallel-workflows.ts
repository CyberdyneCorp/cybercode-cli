// Start several Workflow Runs at once, attach to every active run of the Location (including
// runs other clients started), print their live progress, and wait for each to end.
//   node --experimental-strip-types examples/parallel-workflows.ts audit triage
import { Cyber, WorkflowRunError, type WorkflowRunHandle, type WorkflowRunProgress } from "@cyber-code/sdk";

const client = Cyber.connect({ directory: process.cwd() });
const names = process.argv.slice(2);

// Each run gets at most two of the server-wide agent slots, so no run starves the others.
for (const name of names) {
  const run = await client.workflows.run({ name, max_concurrent: 2 });
  console.log(`started ${run.id} (${name})${run.run.status === "queued" ? `, #${run.run.queue_position} in queue` : ""}`);
}

const active: WorkflowRunHandle[] = [];
for await (const run of client.workflows.list({ status: ["running", "queued", "waiting_input"] })) {
  const { name, status, agents } = run.summary!;
  console.log(`${run.id}  ${name}  ${status}  ${agents.completed}/${agents.total} agents`);
  active.push(run);
}

const controller = new AbortController();
const progress = (async () => {
  for await (const event of client.events.subscribe({ signal: controller.signal })) {
    if (event.type !== "workflow.run.progress") continue;
    const { run_id, status, agents } = event.data as WorkflowRunProgress;
    console.log(`${run_id}  ${status}  ${agents.completed}/${agents.total} agents`);
  }
})().catch(() => {});

const outcomes = await Promise.allSettled(active.map((run) => run.result()));
controller.abort();
await progress;
outcomes.forEach((outcome, i) => {
  const id = active[i]!.id;
  if (outcome.status === "fulfilled") console.log(`${id}: ${JSON.stringify(outcome.value)}`);
  else console.error(`${id}: ${outcome.reason instanceof WorkflowRunError ? outcome.reason.message : String(outcome.reason)}`);
});
