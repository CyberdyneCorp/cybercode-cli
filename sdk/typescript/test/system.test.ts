import assert from "node:assert/strict";
import { test } from "node:test";
import type { Machine } from "../src/generated/types.js";
import { json, mockClient } from "./support.js";

test("machine stats are read for the Location with authentication", async () => {
  const directory = "/project with spaces";
  const machine: Machine = {
    hostname_label: "macOS 15.0",
    cores_logical: 12,
    cores_physical: 10,
    cpu_idle_percent: null,
    load_avg: [2.1, 1.8, 1.6],
    memory_total: 17179869184,
    memory_available: 4294967296,
    disk: { mount: "/", total: 994294398976, available: 16106127360 },
    sampled_at: 1792159320000,
  };
  const location = { directory, project: { id: "global", directory } };
  const { client, calls } = mockClient(() => json(200, { location, data: machine }), { directory });
  const stats = await client.system.stats();
  assert.deepEqual(stats.data, machine);
  assert.equal(stats.data.cpu_idle_percent, null);
  assert.equal(calls[0]?.method, "GET");
  assert.equal(calls[0]?.url.pathname, "/api/v1/system");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent(directory));
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
});
