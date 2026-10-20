// `client.teams`: the generated team operations call the team routes with their path,
// query and body, and team errors have their guards.
import assert from "node:assert/strict";
import { test } from "node:test";
import { isFeatureDisabledError, isTaskClaimedError, isTeamLimitError, isTeamNotFoundError } from "../src/index.js";
import type { Team, TeamView } from "../src/generated/types.js";
import { json, mockClient } from "./support.js";

const team: Team = {
  id: "team_1",
  name: "feature",
  lead_id: "ses_lead",
  directory: "/repo",
  status: "stopped",
  stop_reason: "user",
  created_ms: 1,
  updated_ms: 2,
};

test("teams are listed, shown, stopped and resumed through their routes", async () => {
  const view: TeamView = {
    team: { ...team, status: "active", stop_reason: undefined },
    spent: { turns: 2, tokens: 220, cost_usd: 0.01, wall_seconds: 3 },
    members: [
      {
        team_id: "team_1", name: "api", session_id: "ses_api", role: "teammate", agent: "build",
        isolation: "shared", owns: [], status: "working", turns: 1, tokens: 110, cost_usd: 0.005,
        current_task: { id: "task_1", title: "schema" },
        last_message: { direction: "out", peer: "lead", outcome: "delivered", text: "schema ready", created_ms: 5 },
      },
    ],
    tasks: [],
  };
  const { client, calls } = mockClient((req) => {
    if (req.url.pathname === "/api/v1/teams") return json(200, { location: { directory: "/repo" }, data: [] });
    if (req.url.pathname === "/api/v1/teams/team_1") return json(200, { data: view });
    if (req.url.pathname.endsWith("/tasks")) return json(200, { data: [] });
    return json(200, { data: team });
  }, { directory: "/repo" });
  await client.teams.list({ status: "active" });
  const shown = await client.teams.get("team_1");
  assert.equal(shown.members[0]?.current_task?.title, "schema");
  assert.equal(shown.members[0]?.last_message?.text, "schema ready");
  await client.teams.tasks("team_1", { status: "in_progress", owner: "api" });
  await client.teams.stop("team_1");
  await client.teams.resume("team_1", { lead: "ses_new" });
  assert.deepEqual(
    calls.map((c) => `${c.method} ${c.url.pathname}${c.url.search}`),
    [
      "GET /api/v1/teams?status=active",
      "GET /api/v1/teams/team_1",
      "GET /api/v1/teams/team_1/tasks?status=in_progress&owner=api",
      "POST /api/v1/teams/team_1/stop",
      "POST /api/v1/teams/team_1/resume",
    ],
  );
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent("/repo"));
  assert.deepEqual(JSON.parse(calls[4]?.body ?? "{}"), { lead: "ses_new" });
});

test("a team is created from teammates and its errors have guards", async () => {
  const { client, calls } = mockClient((_req, index) =>
    index === 0
      ? json(404, { _tag: "FeatureDisabledError", message: "Agent teams are disabled." })
      : index === 1
        ? json(409, { _tag: "TeamLimitError", message: "TeamLimitError: Team size limit reached (6)" })
        : index === 2
          ? json(404, { _tag: "TeamNotFoundError", message: "TeamNotFoundError: No team team_x" })
          : json(409, { _tag: "TaskClaimedError", message: "TaskClaimedError: Task already claimed by api" }),
  );
  const body = { lead_session_id: "ses_lead", teammates: [{ name: "api", isolation: "shared" as const }] };
  await assert.rejects(client.teams.create(body), (err) => isFeatureDisabledError(err));
  await assert.rejects(client.teams.create(body), (err) => isTeamLimitError(err));
  await assert.rejects(client.teams.get("team_x"), (err) => isTeamNotFoundError(err));
  await assert.rejects(client.teams.tasks("team_x"), (err) => isTaskClaimedError(err));
  assert.equal(calls[0]?.method, "POST");
  assert.equal(calls[0]?.url.pathname, "/api/v1/teams");
  assert.deepEqual(JSON.parse(calls[0]?.body ?? "{}"), body);
});
