export const meta = {
  name: "migrate",
  description: "File-by-file migration in batches: each batch is edited in its own worktree, checked with your commands and committed on its branch, then verified. Nothing is merged. Non-interactive runs need allow rules for edit and for the check commands.",
  phases: [{ title: "Select" }, { title: "Transform" }, { title: "Verify" }, { title: "Report" }],
  prepare: "migrate_permissions",
  tools: ["glob", "grep"],
  args: {
    type: "object",
    properties: {
      instructions: { type: "string", minLength: 1, description: "What to change in every selected file" },
      files: { type: "array", items: { type: "string", minLength: 1 }, minItems: 1 },
      pattern: { type: "string", minLength: 1, description: "Glob of the files to migrate" },
      contains: { type: "string", minLength: 1, description: "Regular expression a selected file must match" },
      batch_size: { type: "integer", minimum: 1, maximum: 25, default: 5 },
      check: { type: "array", items: { type: "string", minLength: 1 }, default: [] },
      confirm_over: { type: "integer", minimum: 0, default: 20 },
      max_batches: { type: "integer", minimum: 1, maximum: 40, default: 40 },
      yes: { type: "boolean", default: false },
      answers: { type: "object" },
    },
    required: ["instructions"],
    oneOf: [{ required: ["files"] }, { required: ["pattern"] }],
    additionalProperties: false,
  },
  budget: { max_agents: 80, max_cost_usd: 15, max_wall_seconds: 14400 },
  estimate: "two agents per batch: one edits in its worktree, one verifies",
};

const CONFIRM = "Migrate {files} files in {batches} batches?";

export default async function migrate() {
  const state = newState();
  const batchSize = intArg(args.batch_size, 5, 1, 25);
  const checks = Array.isArray(args.check) ? args.check : [];

  phase("Select");
  const selected = await select(state);
  const batches = capped(
    state,
    chunk(selected, batchSize),
    intArg(args.max_batches, 40, 1, 40),
    "batches over max_batches were not migrated",
  );
  const files = batches.reduce((count, batch) => count + batch.length, 0);
  log({ files, batches: batches.length });
  if (files === 0) return envelope(state, "No files matched; nothing was migrated", report([], []));
  const confirmOver = Number.isInteger(args.confirm_over) ? args.confirm_over : 20;
  if (files > confirmOver && args.yes !== true) {
    const question = CONFIRM.replace("{files}", files).replace("{batches}", batches.length);
    const answer = await ask({ question, options: ["Yes", "No"] });
    if (answer !== "Yes") return envelope(state, "Cancelled: " + question + " was answered " + JSON.stringify(answer), report([], []));
  }

  phase("Transform");
  const edits = await parallel(batches.map((batch, index) => () => transform(state, batch, index, checks)));

  phase("Verify");
  const verdicts = await parallel(batches.map((batch, index) => () => verify(state, batch, index, edits[index])));

  phase("Report");
  const rows = batches.map((batch, index) => row(batch, index, edits[index], verdicts[index]));
  const verified = rows.filter((item) => item.verified).length;
  const summary = verified + " of " + rows.length + " batches verified across " + files + " files; nothing was merged";
  return envelope(state, summary, report(rows, rows.filter((item) => item.verified && item.branch)));
}

// The files to migrate, sorted: `files`, or the `pattern` glob, filtered by `contains`.
async function select(state) {
  let paths;
  if (Array.isArray(args.files)) {
    paths = args.files.filter((path) => {
      if (safePath(path)) return true;
      limit(state, "skipped unsafe path " + JSON.stringify(path));
      return false;
    });
  } else {
    const listed = await tools.glob({ pattern: args.pattern, max: 10000 });
    if (listed.truncated) limit(state, "the pattern matched more than 10000 files; only the first 10000 were considered");
    paths = listed.paths;
  }
  if (typeof args.contains === "string") {
    const found = await tools.grep({ pattern: args.contains, include: typeof args.pattern === "string" ? args.pattern : undefined, max: 1000 });
    if (found.truncated) limit(state, "contains saw only the first 1000 matching lines");
    const matching = new Set(found.matches.map((match) => match.path));
    paths = paths.filter((path) => matching.has(path));
  }
  return Array.from(new Set(paths)).sort();
}

function chunk(items, size) {
  const chunks = [];
  for (let i = 0; i < items.length; i += size) chunks.push(items.slice(i, i + size));
  return chunks;
}

function transformSchema() {
  return {
    type: "object",
    properties: {
      status: { type: "string", enum: ["done", "failed"] },
      changed: { type: "array", items: { type: "string" } },
      check: {
        type: "array",
        items: {
          type: "object",
          properties: { command: { type: "string" }, exit_code: { type: "integer" }, tail: { type: "string" } },
          required: ["command", "exit_code", "tail"],
          additionalProperties: false,
        },
      },
      notes: { type: "string" },
    },
    required: ["status", "changed", "check", "notes"],
    additionalProperties: false,
  };
}

// One edit-capable agent per batch in its own worktree: it edits only the batch's files,
// runs the checks there and commits on its branch.
function transform(state, batch, index, checks) {
  const commands = checks.length > 0
    ? "After editing, run each of these commands in your worktree with bash and report its exit code and the last lines of its output:\n" +
      dataBlock("the check commands, one per line", checks.join("\n"))
    : "No check commands were given; report an empty check list.";
  return guarded(state, "batch " + (index + 1), () =>
    agent(
      task(
        "migrate batch " + (index + 1),
        "Apply the migration below to these files only, and change nothing else.\n\n" +
          dataBlock("the migration instructions", args.instructions) + "\n\n" +
          dataBlock("the files of this batch, one per line", batch.join("\n")) + "\n\n" + commands + "\n\n" +
          "Then commit your changes on your branch with a message describing the migration. Report status failed if you could not complete the batch or any check failed.",
      ),
      {
        agent: "general",
        model: "fast",
        isolation: "worktree",
        schema: transformSchema(),
        label: "migrate b" + (index + 1),
        phase: "Transform",
      },
    ),
  );
}

function verifySchema() {
  return {
    type: "object",
    properties: { ok: { type: "boolean" }, problems: { type: "array", items: { type: "string" } } },
    required: ["ok", "problems"],
    additionalProperties: false,
  };
}

// A read-only verifier per edited batch: it reads the branch's diff and the check output.
function verify(state, batch, index, edit) {
  if (edit === null || typeof edit.branch !== "string") return Promise.resolve(null);
  return guarded(state, "verify batch " + (index + 1), () =>
    agent(
      task(
        "verify batch " + (index + 1),
        "Check that branch " + JSON.stringify(edit.branch) + " applies the migration below correctly to these files and changes nothing else. " +
          "Inspect it with `git diff HEAD..." + edit.branch + "` and read the files as needed. Set ok to false and list the problems when anything is wrong or a check failed.\n\n" +
          dataBlock("the migration instructions", args.instructions) + "\n\n" +
          dataBlock("the files of this batch, one per line", batch.join("\n")) + "\n\n" +
          dataBlock("the editing agent's report", JSON.stringify({ status: edit.status, changed: edit.changed, check: edit.check, notes: edit.notes }, null, 2)),
      ),
      { agent: "reviewer", model: "verify", schema: verifySchema(), label: "verify b" + (index + 1), phase: "Verify" },
    ),
  );
}

// A batch is `done` only when its agent finished and every check exited 0, and `verified`
// only when it is done and its verifier found no problem.
function row(batch, index, edit, verdict) {
  const checks = edit === null ? [] : (edit.check || []).map((check) => ({ command: check.command, exit_code: check.exit_code }));
  const checksPassed = checks.every((check) => check.exit_code === 0);
  const status = edit === null ? "not_run" : edit.status === "done" && checksPassed ? "done" : "failed";
  const problems = verdict === null ? [] : verdict.problems || [];
  return {
    index: index + 1,
    files: batch,
    branch: edit === null ? null : edit.branch || null,
    status,
    checks,
    verified: status === "done" && verdict !== null && verdict.ok === true,
    problems,
  };
}

function report(batches, mergeable) {
  return {
    batches,
    merged: false,
    integrate: mergeable.length === 0 ? null : "git merge " + mergeable.map((item) => item.branch).join(" "),
  };
}
