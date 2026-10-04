// Follow a Session's durable events, resuming after the last seen sequence number.
//   node --experimental-strip-types examples/stream-events.ts ses_... [after]
import { Cyber } from "@cyber-code/sdk";

const [sessionID, after] = process.argv.slice(2);
if (!sessionID) throw new Error("usage: stream-events.ts <sessionID> [after]");

const client = Cyber.connect();
const controller = new AbortController();
process.once("SIGINT", () => controller.abort());

// The iterator reconnects with backoff (500 ms → 15 s) and resumes from the last `durable.seq`,
// so no durable event is lost or repeated. Persist `lastSeq` to resume across process restarts.
let lastSeq = after === undefined ? undefined : Number(after);
for await (const event of client.sessions.events(sessionID, { after: lastSeq, signal: controller.signal })) {
  lastSeq = event.durable?.seq ?? lastSeq;
  console.log(`${String(lastSeq).padStart(6)} ${event.type}`);
}
console.log(`stopped after seq ${lastSeq}`);
