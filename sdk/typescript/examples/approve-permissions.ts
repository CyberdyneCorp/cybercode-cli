// Approve read-only permission requests once, reject everything else, until Ctrl-C.
//   node --experimental-strip-types examples/approve-permissions.ts [directory]
import { Cyber, type PermissionDecision, type PermissionRequest } from "@cyber-code/sdk";

const client = Cyber.connect().at(process.argv[2] ?? process.cwd());
const READ_ONLY = new Set(["read", "glob", "grep", "list"]);

function decide(request: PermissionRequest): PermissionDecision {
  if (READ_ONLY.has(request.action)) return "once";
  return { reply: "reject", message: `${request.action} is not allowed by approve-permissions.ts` };
}

const stop = client.permissions.onRequest(
  (request) => {
    const decision = decide(request);
    console.log(`${request.session_id} ${request.action} ${request.resources.join(" ")} -> ${JSON.stringify(decision)}`);
    return decision;
  },
  { onError: (err) => console.error("reply failed:", err) },
);

process.once("SIGINT", () => {
  stop();
  console.log("stopped");
});
console.log(`answering permission requests for ${client.directory}; Ctrl-C to stop`);
