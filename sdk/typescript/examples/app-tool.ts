// Register a tool that runs in this process, so the model can call it, until Ctrl-C.
//   node --experimental-strip-types examples/app-tool.ts
import { Cyber, type AppTool } from "@cyber-code/sdk";

const TICKETS: Record<string, string> = { "T-1": "Login page returns 500 after deploy", "T-2": "Dark mode toggle ignored" };

const lookupTicket: AppTool<{ id: string }> = {
  name: "lookup_ticket",
  description: "Look up an issue-tracker ticket by ID and return its title.",
  input: { type: "object", properties: { id: { type: "string", description: "Ticket ID, e.g. T-1" } }, required: ["id"] },
  execute: ({ id }, { sessionID }) => {
    console.log(`${sessionID}: lookup_ticket ${id}`);
    const title = TICKETS[id];
    if (!title) throw new Error(`no ticket ${id}`); // Reported to the model as the tool's failure.
    return `${id}: ${title}`;
  },
};

const client = Cyber.connect();
const unregister = await client.tools.register(lookupTicket);
const listed = await client.tool.list();
console.log(`registered; advertised: ${listed.data.some((t) => t.name === lookupTicket.name)}. Ctrl-C to stop`);

process.once("SIGINT", () => void unregister().then(() => console.log("unregistered")));
