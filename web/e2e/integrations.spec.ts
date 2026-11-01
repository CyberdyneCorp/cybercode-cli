import { spawnSync } from "node:child_process";
import { readFile, writeFile, readdir } from "node:fs/promises";
import { join, dirname } from "node:path";
import { test, expect, login, select, tab } from "./browser.js";
import { eventually, hostile } from "./fixture.js";

for (const navigateAfterSubmit of [false, true]) {
  test(`integration queue verifies actual combined source and renders evidence without publishing${navigateAfterSubmit ? " after navigation" : ""}`, async ({
    dashboard,
    page,
  }) => {
    const git = (args: string[]) => {
      const result = spawnSync("git", args, {
        cwd: dashboard.repo,
        env: {
          PATH: process.env.PATH,
          HOME: join(dashboard.root, "home"),
          GIT_CONFIG_NOSYSTEM: "1",
          GIT_CONFIG_GLOBAL: join(dashboard.root, "empty-git-config"),
        },
        encoding: "utf8",
      });
      expect(result.status, result.stderr).toBe(0);
      return result.stdout.trim();
    };
    const target = git(["symbolic-ref", "--short", "HEAD"]);
    git(["checkout", "--quiet", "-b", "integration-contributor"]);
    await writeFile(join(dashboard.repo, "contribution.txt"), hostile);
    git(["add", "contribution.txt"]);
    git(["commit", "--quiet", "-m", "browser contribution"]);
    const commit = git(["rev-parse", "HEAD"]);
    git(["checkout", "--quiet", target]);
    const configPath = join(dashboard.home, "config/cyber.json");
    const config = JSON.parse(await readFile(configPath, "utf8"));
    config.permissions = {
      bash: "allow",
      read: "allow",
      edit: "allow",
      worktree: "allow",
      external_directory: "allow",
    };
    await writeFile(configPath, JSON.stringify(config));
    const owner = await dashboard.session("Integration owner");
    await login(page, dashboard);
    await select(page, "Integration owner");
    await tab(page, "Integrations");
    const publications: string[] = [];
    page.on("request", (request) => {
      if (request.url().endsWith("/publish")) publications.push(request.url());
    });
    await page.getByLabel("Integration manifest (JSON)", { exact: true }).fill(
      JSON.stringify({
        target_branch: target,
        contributions: [{ label: "browser", commit }],
        checks: [{ argv: ["true"] }],
        reviewer_model: "missing/model",
      }),
    );
    const submitted = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname.endsWith("/workflows/integrations"),
      { timeout: 15000 },
    );
    await page
      .getByRole("button", { name: "Submit integration", exact: true })
      .click();
    const submitResponse = await submitted;
    if (navigateAfterSubmit) await page.goto("about:blank");
    // Chromium may evict the body on navigation; status metadata remains available.
    expect(submitResponse.ok(), `Integration submission returned HTTP ${submitResponse.status()}`).toBe(true);
    const queued = await eventually(
      () =>
        dashboard.api<{ id: string; status: string; session_id: string }[]>(
          "/workflows/integrations",
        ),
      (rows) => rows.some((row) => row.session_id === owner),
    );
    const batch = queued.find((row) => row.session_id === owner)!;
    if (navigateAfterSubmit) {
      await login(page, dashboard);
      await select(page, "Integration owner");
      await tab(page, "Integrations");
      await page.getByRole("button", { name: new RegExp(`^${batch.id} ·`) }).click();
    }
    await expect(
      page.getByRole("heading", { name: new RegExp(batch.id) }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: "Run a new attempt", exact: true })
      .click();
    await eventually(
      () =>
        dashboard.api<{ status: string }>(`/workflows/integrations/${batch.id}`),
      (row) => row.status === "ready",
      30_000,
    );
    await page
      .getByRole("button", { name: "Refresh selected evidence", exact: true })
      .click();
    await expect(
      page.locator("pre", { hasText: "review_unavailable" }),
    ).toContainText('"outcome": "passed"');
    await expect(
      page.locator("pre", { hasText: "review_unavailable" }),
    ).toContainText(hostile.split("\n")[0]!);
    await expect(
      page.getByRole("button", {
        name: "Publish reviewed integration",
        exact: true,
      }),
    ).toBeDisabled();
    expect(publications).toEqual([]);
    expect(await page.locator("iframe").count()).toBe(0);
    expect(await page.evaluate(() => "__dashboardPwned" in globalThis)).toBe(
      false,
    );
    await page
      .getByRole("button", { name: "Cancel integration", exact: true })
      .click();
    await eventually(
      () =>
        dashboard.api<{ status: string }>(`/workflows/integrations/${batch.id}`),
      (row) => row.status === "cancelled",
    );
  });
}

test("a server crash during native verification remains unknown and never reexecutes the check", async ({
  dashboard,
}) => {
  test.skip(
    process.platform === "win32",
    "The process-crash fixture uses POSIX sh and sleep.",
  );
  const configPath = join(dashboard.home, "config/cyber.json");
  const config = JSON.parse(await readFile(configPath, "utf8"));
  config.permissions = {
    bash: "allow",
    read: "allow",
    edit: "allow",
    worktree: "allow",
    external_directory: "allow",
  };
  await writeFile(configPath, JSON.stringify(config));
  const git = (args: string[]) => {
    const result = spawnSync("git", args, {
      cwd: dashboard.repo,
      encoding: "utf8",
    });
    expect(result.status, result.stderr).toBe(0);
    return result.stdout.trim();
  };
  git(["checkout", "--quiet", "-b", "crash-contribution"]);
  await writeFile(join(dashboard.repo, "crash.txt"), "safe source");
  git(["add", "crash.txt"]);
  git(["commit", "--quiet", "-m", "crash contribution"]);
  const commit = git(["rev-parse", "HEAD"]);
  git(["checkout", "--quiet", "main"]);
  const owner = await dashboard.session("Crash owner");
  const batch = await dashboard.api<{ id: string; revision: number }>(
    "/workflows/integrations",
    "POST",
    {
      session_id: owner,
      manifest: {
        contributions: [{ label: "crash", commit }],
        checks: [{ argv: ["sh", "-c", "sleep 30"], timeout_seconds: 60 }],
        reviewer_model: "missing/model",
      },
    },
  );
  const started = await dashboard.api<{ run_id: string }>(
    `/workflows/integrations/${batch.id}/run`,
    "POST",
    { expected_revision: batch.revision },
  );
  await eventually(
    () =>
      dashboard.api<{ status: string }>(`/workflows/integrations/${batch.id}`),
    (row) => row.status === "checking",
  );
  // Wait for the durable intent, rather than merely the JS phase changing to Test.
  const database = (await dashboard.terminal(["db", "path"])).stdout.trim();
  const intentCount = () => {
    const query =
      "import sqlite3,sys; db=sqlite3.connect(sys.argv[1]); print(db.execute(\"SELECT COUNT(*) FROM event WHERE aggregate_id=? AND type='workflow.integration.evidence.1'\",(sys.argv[2],)).fetchone()[0])";
    const result = spawnSync(
      "python3",
      ["-c", query, database, `integration_check_${started.run_id}_0`],
      { encoding: "utf8" },
    );
    if (result.status !== 0) throw Error(result.stderr);
    return Number(result.stdout.trim());
  };
  try {
    await eventually(async () => intentCount(), (count) => count === 1);
  } catch (error) {
    // Preserve the real check outcome when setup/source validation settles before
    // the intended crash point. Do not relax the required single durable intent.
    const batchState = await dashboard.api<{ status: string }>(
      `/workflows/integrations/${batch.id}`,
    );
    const report = await dashboard.api<{
      status: string;
      report?: { host?: { checks?: { outcome?: string; limitation?: string }[]; limitation?: string } };
    }>(`/workflows/integrations/${batch.id}/report`);
    throw new Error(`${String(error)}: ${JSON.stringify({
      evidence_count: intentCount(), batch_status: batchState.status,
      report_status: report.status,
      checks: report.report?.host?.checks?.map((check) => ({
        outcome: check.outcome, limitation: check.limitation?.slice(0, 500),
      })),
      limitation: report.report?.host?.limitation?.slice(0, 500),
    })}`);
  }
  await dashboard.crash();
  await dashboard.start();
  const interrupted = await dashboard.api<{ status: string }>(
    `/workflows/integrations/${batch.id}`,
  );
  expect(interrupted.status).toBe("unknown");
  await dashboard.request(
    `/workflows/runs/${started.run_id}/resume`,
    "POST",
    {},
  );
  await eventually(
    () =>
      dashboard.api<{ status: string }>(`/workflows/runs/${started.run_id}`),
    (run) => run.status === "failed",
  );
  const report = await dashboard.api<{
    status: string;
    report: { host: { eligible: boolean } };
  }>(`/workflows/integrations/${batch.id}/report`);
  expect(report.status).toBe("unknown");
  expect(report.report.host.eligible).toBe(false);
  expect(intentCount()).toBe(1);
  expect(dashboard.modelRequests).toHaveLength(0);
});

for (const publicationGate of ["ack", "lookup", "admission", "uncertain"] as const) {
test(publicationGate === "ack" ? "publication reconciles a lost command acknowledgement without repeating Git or PR creation" : publicationGate === "uncertain" ? "unknown publication receipts remain inspectable without repeating native effects" : `cancellation during publication ${publicationGate} prevents PR creation`, async ({
  dashboard,
}) => {
  test.skip(
    process.platform === "win32",
    "The publication adapter fixture uses POSIX executable shims.",
  );
  const { mkdir, chmod } = await import("node:fs/promises");
  const bin = join(dashboard.home, "tmp", "publisher-bin");
  await mkdir(bin, { recursive: true });
  const gitPath = spawnSync("which", ["git"], {
    encoding: "utf8",
  }).stdout.trim();
  expect(gitPath).not.toBe("");
  await writeFile(
    join(bin, "git"),
    `#!/bin/sh\nif [ -n "\${LD_PRELOAD+x}\${DYLD_INSERT_LIBRARIES+x}\${DYLD_LIBRARY_PATH+x}\${GH_CONFIG_DIR+x}" ]; then printf x > "$TMPDIR/native-env-executed"; exit 79; fi\ncase "\${HOME-}:\${XDG_CONFIG_HOME-}" in *contributed*) exit 79;; esac\nfor arg do\n if [ "$arg" = push ]; then printf x >> "$TMPDIR/push-count"; exit ${publicationGate === "uncertain" ? 76 : 0}; fi\ndone\nexec '${gitPath}' "$@"\n`,
  );
  await chmod(join(bin, "git"), 0o700);
  const configPath = join(dashboard.home, "config/cyber.json");
  const config = JSON.parse(await readFile(configPath, "utf8"));
  config.permissions = {
    bash: "allow",
    read: "allow",
    edit: "allow",
    worktree: "allow",
    external_directory: "allow",
  };
  if (publicationGate === "admission") {
    const hook = join(bin, "wait-create.py");
    await writeFile(hook, `import os,sys,time
sys.stdin.read()
root=os.environ['TMPDIR']
open(os.path.join(root,'admission-started'),'w').close()
while not os.path.exists(os.path.join(root,'release-admission')): time.sleep(.05)
`);
    config.sandbox = {readable_paths:[bin]};
    config.hooks = {PreToolUse:[{matcher:"bash",hooks:[{type:"command", command:`python3 '${hook}'`, if:{field:"tool_input.command",matches:"^'gh' 'pr' 'create' "}}]}]};
  }
  if (publicationGate === "ack") {
    config.sandbox = {env:{set:{LD_PRELOAD:"",DYLD_INSERT_LIBRARIES:"",DYLD_LIBRARY_PATH:"./contributed-libraries",GH_CONFIG_DIR:"./contributed-gh",HOME:"./contributed-home",XDG_CONFIG_HOME:"./contributed-config"}}};
  }
  await writeFile(configPath, JSON.stringify(config));
  await dashboard.restartWithTrustedPath(bin);
  const git = (args: string[]) => {
    const result = spawnSync(gitPath, args, {
      cwd: dashboard.repo,
      encoding: "utf8",
    });
    expect(result.status, result.stderr).toBe(0);
    return result.stdout.trim();
  };
  git([
    "remote",
    "add",
    "origin",
    "https://github.com/fixture/integration.git",
  ]);
  git(["checkout", "--quiet", "-b", "publication-contributor"]);
  await writeFile(
    join(dashboard.repo, "publication.txt"),
    "owner selected change",
  );
  git(["add", "publication.txt"]);
  git(["commit", "--quiet", "-m", "publication contribution"]);
  const commit = git(["rev-parse", "HEAD"]);
  git(["checkout", "--quiet", "main"]);
  const owner = await dashboard.session("Publication owner");
  const batch = await dashboard.api<{ id: string; revision: number }>(
    "/workflows/integrations",
    "POST",
    {
      session_id: owner,
      manifest: {
        contributions: [{ label: "publish", commit }],
        checks: [{ argv: ["true"] }],
        reviewer_model: "missing/model",
      },
    },
  );
  await dashboard.api(`/workflows/integrations/${batch.id}/run`, "POST", {
    expected_revision: batch.revision,
  });
  type Ready = {
    id: string;
    revision: number;
    run_id: string;
    status: string;
    report: {
      host: {
        branch: string;
        combined_commit: string;
        target_branch: string;
        eligible: boolean;
      };
    };
  };
  const ready = await eventually(
    () => dashboard.api<Ready>(`/workflows/integrations/${batch.id}`),
    (row) => row.status === "ready",
  );
  expect(ready.report.host.eligible).toBe(true);
  const row = {
    number: 7,
    url: "https://github.com/fixture/integration/pull/7",
    headRefName: ready.report.host.branch,
    headRefOid: ready.report.host.combined_commit,
    baseRefName: ready.report.host.target_branch,
    isCrossRepository: false,
  };
  await writeFile(
    join(bin, "gh"),
    `#!/bin/sh\nif [ -n "\${LD_PRELOAD+x}\${DYLD_INSERT_LIBRARIES+x}\${DYLD_LIBRARY_PATH+x}\${GH_CONFIG_DIR+x}" ]; then printf x > "$TMPDIR/native-env-executed"; exit 79; fi\ncase "\${HOME-}:\${XDG_CONFIG_HOME-}" in *contributed*) exit 79;; esac\nif [ "$2" = list ]; then\n if [ -f "$TMPDIR/pr.json" ]; then cat "$TMPDIR/pr.json"; else printf '[]'; fi\nelse\n printf x >> "$TMPDIR/create-count"\n printf '%s' '${JSON.stringify([row])}' > "$TMPDIR/pr.json"\n printf 'response lost after remote creation' >&2\n exit 76\nfi\n`,
  );
  await chmod(join(bin, "gh"), 0o700);
  if (publicationGate !== "ack") {
    const wait = publicationGate === "lookup" ? `printf x > "$TMPDIR/lookup-started"
while [ ! -f "$TMPDIR/release-lookup" ]; do sleep .05; done
` : "";
    await writeFile(join(bin,"gh"), `#!/bin/sh
if [ "$2" = list ]; then
${wait}printf '[]'
else
 printf x >> "$TMPDIR/create-count"
 exit 76
fi
`);
  }
  const input = {
    expected_revision: ready.revision,
    operation_id: "publication-lost-ack",
    remote: "origin",
    repository: "fixture/integration",
    title: "Reviewed integration",
    body: "Real host checks, mocked remote transport only.",
    acknowledge_review_unavailable: true,
  };
  if (publicationGate === "uncertain") {
    const result = await dashboard.request(`/workflows/integrations/${batch.id}/publish`,"POST",input);
    expect(result.ok).toBe(false);
    for (const suffix of ["","/report"]) {
      const report = await dashboard.api<{publication:{phase:string;last_phase:string;repository:string;destination:string;records:{data:{phase:string;outcome?:{settled:boolean;outcome:string}}}[]}}>(`/workflows/integrations/${batch.id}${suffix}`);
      expect(report.publication.phase).toBe("unknown");
      expect(report.publication.last_phase).toBe("push");
      expect(report.publication.repository).toBe("fixture/integration");
      expect(report.publication.destination).toBe("https://github.com/fixture/integration.git");
      expect(report.publication.records.at(-1)?.data.outcome?.settled).toBe(true);
      expect(report.publication.records.at(-1)?.data.outcome?.outcome).toBe("failed");
    }
    const paths = JSON.parse((await dashboard.terminal(["debug","paths","--format","json"])).stdout) as {tmp:string};
    const temp = join(paths.tmp,"sessions",`i-${ready.run_id.replace(/^run_/, "")}`);
    expect(await readFile(join(temp,"push-count"),"utf8")).toBe("x");
    expect(await readFile(join(temp,"create-count"),"utf8").catch(()=>"absent")).toBe("absent");
    return;
  }
  if (publicationGate !== "ack") {
    const paths = JSON.parse((await dashboard.terminal(["debug","paths","--format","json"])).stdout) as {tmp:string};
    const temp = join(paths.tmp,"sessions",`i-${ready.run_id.replace(/^run_/, "")}`);
    const publishing = dashboard.request(`/workflows/integrations/${batch.id}/publish`,"POST",input).then(response=>({response}),error=>({error}));
    const gateTemp = await eventually(async()=>{
      const scratch = (await readdir(paths.tmp,{recursive:true})).filter(name=>name.endsWith(`${publicationGate}-started`)).map(name=>dirname(join(paths.tmp,name)));
      for (const candidate of [temp,join(paths.tmp,"sessions",owner),...scratch]) {
        if (await readFile(join(candidate,`${publicationGate}-started`),"utf8").then(()=>true).catch(()=>false)) return candidate;
      }
      return "";
    },Boolean);
    await dashboard.api(`/workflows/integrations/${batch.id}/cancel`,"POST",{expected_revision:ready.revision});
    await writeFile(join(gateTemp,`release-${publicationGate}`),"continue");
    const outcome = await publishing;
    if ("error" in outcome) throw outcome.error;
    expect(outcome.response.ok).toBe(false);
    expect(await readFile(join(temp,"create-count"),"utf8").catch(()=>"absent")).toBe("absent");
    const cancelled = await dashboard.api<{status:string}>(`/workflows/integrations/${batch.id}`);
    expect(cancelled.status).toBe("cancelled");
    return;
  }
  const published = await dashboard.api<{
    revision: number;
    publication: { phase: string };
  }>(`/workflows/integrations/${batch.id}/publish`, "POST", input);
  expect(published.publication.phase).toBe("published");
  await dashboard.api(`/workflows/integrations/${batch.id}/publish`, "POST", {
    ...input,
    expected_revision: published.revision,
  });
  const paths = JSON.parse(
    (await dashboard.terminal(["debug", "paths", "--format", "json"])).stdout,
  ) as { tmp: string };
  const temp = join(paths.tmp, "sessions", `i-${ready.run_id.replace(/^run_/, "")}`);
  expect(await readFile(join(temp, "push-count"), "utf8")).toBe("x");
  expect(await readFile(join(temp, "create-count"), "utf8")).toBe("x");
  expect(await readFile(join(temp,"native-env-executed"),"utf8").catch(()=>"absent")).toBe("absent");
});

}
