// Real owner service + local Git + generated queue operations. No account login or remote publication.
import assert from "node:assert/strict";
import {
  mkdtempSync,
  realpathSync,
  readFileSync,
  writeFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import {
  cloneMachines,
  configureMachine,
  execute,
  startMachine,
} from "./collaboration-processes.js";
import { startScriptedProvider } from "./scripted-provider.js";
const binary = process.env.CYBER_BIN;
test(
  "generated integration SDK verifies combined local commits through the real server",
  {
    skip: binary
      ? false
      : "set CYBER_BIN for actual integration queue acceptance",
    timeout: 60000,
  },
  async (t) => {
    const root = realpathSync(
      mkdtempSync(join(tmpdir(), "cyber-sdk-integrations-")),
    );
    const model = await startScriptedProvider();
    const clones = await cloneMachines(root, ["owner"]);
    const repo = clones.get("owner")!;
    const env = configureMachine(
      root,
      "owner",
      "http://127.0.0.1:9",
      model.config(),
    );
    const configPath = join(env.CYBER_HOME!, "config/cyber.json");
    const config = JSON.parse(readFileSync(configPath, "utf8"));
    config.features = { collaboration: false };
    config.permissions = {
      bash: "allow",
      edit: "allow",
      read: "allow",
      worktree: "allow",
      external_directory: "allow",
    };
    writeFileSync(configPath, JSON.stringify(config));
    const target = (
      await execute("git", ["symbolic-ref", "--short", "HEAD"], env, repo)
    ).trim();
    await execute("git", ["checkout", "-qb", "contribution"], env, repo);
    writeFileSync(join(repo, "integrated.txt"), "SDK contribution\n");
    await execute("git", ["add", "integrated.txt"], env, repo);
    await execute(
      "git",
      [
        "-c",
        "user.name=SDK fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-qm",
        "SDK contribution",
      ],
      env,
      repo,
    );
    const commit = (
      await execute("git", ["rev-parse", "HEAD"], env, repo)
    ).trim();
    await execute("git", ["checkout", "-q", target], env, repo);
    const server = await startMachine(binary!, env, repo);
    t.after(async () => {
      await server.close();
      await model.close();
      rmSync(root, { recursive: true, force: true });
    });
    const owner = (
      await server.client.session.create({ title: "Integration SDK owner" })
    ).data.id;
    const batch = await server.client.workflow.integrationSubmit({
      session_id: owner,
      manifest: {
        contributions: [{ label: "sdk", commit, depends_on: [] }],
        checks: [{ argv: ["true"], timeout_seconds: 30 }],
        reviewer_model: "missing/model",
      },
    });
    assert.equal(batch.status, "queued");
    await server.client.workflow.integrationRun(batch.id, {
      expected_revision: batch.revision,
    });
    let report = await server.client.workflow.integrationReport(batch.id);
    const deadline = Date.now() + 30000;
    while (
      ["queued", "preparing", "checking", "reviewing"].includes(
        report.status,
      ) &&
      Date.now() < deadline
    ) {
      await new Promise((resolve) => setTimeout(resolve, 50));
      report = await server.client.workflow.integrationReport(batch.id);
    }
    assert.equal(report.status, "ready", JSON.stringify(report));
    const evidence = report.report as {
      host: {
        eligible: boolean;
        checks: Array<{ outcome: string; settled: boolean }>;
      };
      workflow: { review_status: string };
    };
    assert.equal(evidence.host.eligible, true);
    assert.equal(evidence.host.checks[0]?.outcome, "passed");
    assert.equal(evidence.host.checks[0]?.settled, true);
    assert.equal(evidence.workflow.review_status, "review_unavailable");
    const cancelled = await server.client.workflow.integrationCancel(batch.id, {
      expected_revision: report.revision,
    });
    assert.equal(cancelled.status, "cancelled");
    assert.equal((await server.client.workflow.integrationList()).length, 1);
  },
);
