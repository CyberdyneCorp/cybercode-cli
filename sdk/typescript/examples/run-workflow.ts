// Start a saved Workflow (or the inline module given as a file), follow its events and print
// its return value.
//   node --experimental-strip-types examples/run-workflow.ts audit '{"base":"main"}'
import { readFileSync } from "node:fs";
import { Cyber, WorkflowRunError } from "@cyber-code/sdk";

const client = Cyber.connect({ directory: process.cwd() });
const target = process.argv[2] ?? "audit";
const args = JSON.parse(process.argv[3] ?? "{}") as Record<string, unknown>;
const source = target.endsWith(".js") ? { script: readFileSync(target, "utf8") } : { name: target };

const run = await client.workflows.run({ ...source, args, budget: { max_cost_usd: 10 } });
console.log(`run ${run.id} (${run.run.name}) started`);

const progress = (async () => {
  for await (const event of run.events()) {
    console.log(`${event.type}`);
    if (event.type.startsWith("workflow.run.") && !event.type.startsWith("workflow.run.started")) break;
  }
})();

try {
  console.log(JSON.stringify(await run.result(), null, 2));
} catch (err) {
  if (err instanceof WorkflowRunError) console.error(err.message);
  else throw err;
}
await progress;
