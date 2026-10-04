// Create a Session in the current directory, prompt it, and print the final answer.
//   node --experimental-strip-types examples/drive-session.ts "fix the failing test"
import { Cyber, isSessionBusyError } from "@cyber-code/sdk";

const client = Cyber.connect({ directory: process.cwd() });

const { data: session } = await client.session.create({ title: "SDK example" });
console.log(`session ${session.id} (${session.model})`);

try {
  const result = await client.sessions.prompt(session.id, process.argv[2] ?? "Summarize this repository in one paragraph.", {
    wait: true,
  });
  console.log(result.text);
  console.log(`stop: ${result.stopReason}; tokens in/out: ${result.usage.input}/${result.usage.output}`);
} catch (err) {
  if (isSessionBusyError(err)) console.error("the session is busy; try again later");
  else throw err;
}
