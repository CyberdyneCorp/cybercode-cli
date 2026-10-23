export const meta = {
  name: "plan",
  description: "Plans drafted from several independent angles, scored by judges on a fixed rubric, then merged: the best draft with named improvements grafted from the others. Read-only; nothing is edited.",
  phases: [{ title: "Draft" }, { title: "Judge" }, { title: "Merge" }],
  args: {
    type: "object",
    properties: {
      task: { type: "string", minLength: 1 },
      angles: {
        type: "array",
        items: { type: "string", minLength: 1, maxLength: 64 },
        minItems: 2,
        maxItems: 6,
        uniqueItems: true,
        default: ["mvp-first", "risk-first", "user-first"],
      },
      judges: { type: "integer", minimum: 1, maximum: 5, default: 3 },
      context: { type: "array", items: { type: "string", minLength: 1 }, default: [] },
    },
    required: ["task"],
    additionalProperties: false,
  },
  budget: { max_agents: 14, max_cost_usd: 3, max_wall_seconds: 1800 },
  estimate: "one planner per angle + judges + one merge",
};

const CRITERIA = ["correctness", "completeness", "risk", "effort", "testability"];

export default async function plan() {
  const state = newState();
  const angles = Array.isArray(args.angles) && args.angles.length >= 2 ? args.angles.slice(0, 6) : ["mvp-first", "risk-first", "user-first"];
  const judges = intArg(args.judges, 3, 1, 5);

  phase("Draft");
  const drafted = await parallel(angles.map((angle, index) => () => draft(state, angle, index)));
  const drafts = [];
  drafted.forEach((value, index) => {
    if (value !== null) drafts.push({ angle: angles[index], plan: value });
  });
  if (drafts.length === 0) return envelope(state, "No plan could be drafted", { plan_markdown: "", chosen: null, scores: {}, grafts: [], drafts: [] });

  phase("Judge");
  const verdicts = await parallel(Array.from({ length: judges }, (_, index) => () => judge(state, drafts, index)));
  const scores = tally(drafts, verdicts.filter((verdict) => verdict !== null));
  const chosen = drafts
    .map((item) => item.angle)
    .sort((a, b) => scores[b].total - scores[a].total || angles.indexOf(a) - angles.indexOf(b))[0];

  phase("Merge");
  const merged = drafts.length > 1 ? await merge(state, drafts, chosen, scores) : null;
  const winner = drafts.find((item) => item.angle === chosen);
  const grafts = merged === null ? [] : validGrafts(state, merged.grafts || [], drafts, chosen);
  const markdown = merged === null ? render(winner.plan) : merged.plan_markdown;
  const summary = "Plan from the " + chosen + " draft with " + grafts.length + " graft" + (grafts.length === 1 ? "" : "s") + " from " + (drafts.length - 1) + " other drafts";
  return envelope(state, summary, { plan_markdown: markdown, chosen, scores, grafts, drafts });
}

function draftSchema() {
  const list = { type: "array", items: { type: "string" } };
  return {
    type: "object",
    properties: {
      title: { type: "string", minLength: 1 },
      steps: {
        type: "array",
        minItems: 1,
        items: {
          type: "object",
          properties: { step: { type: "string", minLength: 1 }, files: list, risk: { type: "string" } },
          required: ["step", "files", "risk"],
          additionalProperties: false,
        },
      },
      assumptions: list,
      risks: list,
      tests: list,
    },
    required: ["title", "steps", "assumptions", "risks", "tests"],
    additionalProperties: false,
  };
}

function context() {
  const paths = Array.isArray(args.context) ? args.context.filter(safePath) : [];
  return paths.length === 0 ? "" : "\n\nRead these files first:\n" + dataBlock("context paths, one per line", paths.join("\n"));
}

// One read-only planner per angle.
function draft(state, angle, index) {
  return guarded(state, "draft " + angle, () =>
    agent(
      task(
        "draft " + angle,
        "Write an implementation plan for the task below, taking the " + JSON.stringify(angle) + " angle: let it decide what comes first and what is left out. " +
          "Read the code you need; do not edit anything. List concrete steps with the files they touch and their risk, your assumptions, the main risks and the tests that prove it works.\n\n" +
          dataBlock("the task", args.task) + context(),
      ),
      { agent: "reviewer", model: "smart", schema: draftSchema(), label: "draft " + (index + 1), phase: "Draft" },
    ),
  );
}

function judgeSchema() {
  const score = { type: "integer", minimum: 1, maximum: 5 };
  const properties = { angle: { type: "string" }, reasons: { type: "string" } };
  for (const criterion of CRITERIA) properties[criterion] = score;
  return {
    type: "object",
    properties: {
      scores: {
        type: "array",
        items: { type: "object", properties, required: ["angle", "reasons"].concat(CRITERIA), additionalProperties: false },
      },
    },
    required: ["scores"],
    additionalProperties: false,
  };
}

// Each judge scores every draft on the rubric.
function judge(state, drafts, index) {
  return guarded(state, "judge " + (index + 1), () =>
    agent(
      task(
        "judge " + (index + 1),
        "Score every draft plan below from 1 (poor) to 5 (excellent) on correctness, completeness, risk (5 = lowest risk), effort (5 = least effort) and testability, with reasons. Score each draft by its angle name.\n\n" +
          dataBlock("the task", args.task) + "\n\n" + dataBlock("the drafts as JSON", JSON.stringify(drafts, null, 2)),
      ),
      { agent: "reviewer", model: "verify", schema: judgeSchema(), label: "judge " + (index + 1), phase: "Judge" },
    ),
  );
}

// Per angle: the summed score of every criterion over the judges, and the total.
function tally(drafts, verdicts) {
  const scores = {};
  for (const item of drafts) {
    const byCriterion = {};
    for (const criterion of CRITERIA) byCriterion[criterion] = 0;
    scores[item.angle] = { total: 0, by_criterion: byCriterion };
  }
  for (const verdict of verdicts) {
    for (const row of verdict.scores || []) {
      const entry = scores[row.angle];
      if (entry === undefined) continue;
      for (const criterion of CRITERIA) {
        entry.by_criterion[criterion] += row[criterion];
        entry.total += row[criterion];
      }
    }
  }
  return scores;
}

function mergeSchema() {
  return {
    type: "object",
    properties: {
      plan_markdown: { type: "string", minLength: 1 },
      grafts: {
        type: "array",
        items: {
          type: "object",
          properties: { from: { type: "string" }, what: { type: "string", minLength: 1 } },
          required: ["from", "what"],
          additionalProperties: false,
        },
      },
    },
    required: ["plan_markdown", "grafts"],
    additionalProperties: false,
  };
}

function merge(state, drafts, chosen, scores) {
  return guarded(state, "merge", () =>
    agent(
      task(
        "merge plans",
        "The " + JSON.stringify(chosen) + " draft scored highest. Write the final plan in Markdown from it, grafting in the specific improvements from the other drafts that make it better. " +
          "List every graft with the angle of the draft it came from and what it adds.\n\n" +
          dataBlock("the task", args.task) + "\n\n" + dataBlock("the drafts as JSON", JSON.stringify(drafts, null, 2)) + "\n\n" +
          dataBlock("the scores as JSON", JSON.stringify(scores, null, 2)),
      ),
      { agent: "reviewer", model: "smart", schema: mergeSchema(), label: "merge", phase: "Merge" },
    ),
  );
}

// Grafts must name another draft's angle.
function validGrafts(state, grafts, drafts, chosen) {
  const others = drafts.map((item) => item.angle).filter((angle) => angle !== chosen);
  return grafts.filter((graft) => {
    if (others.indexOf(graft.from) >= 0) return true;
    limit(state, "dropped a graft naming an unknown draft " + JSON.stringify(graft.from));
    return false;
  });
}

function render(draft) {
  const lines = ["# " + draft.title, ""];
  draft.steps.forEach((step, index) => {
    const files = step.files.length > 0 ? " (" + step.files.join(", ") + ")" : "";
    lines.push(index + 1 + ". " + step.step + files + (step.risk ? " — risk: " + step.risk : ""));
  });
  for (const [title, items] of [["Assumptions", draft.assumptions], ["Risks", draft.risks], ["Tests", draft.tests]]) {
    if (items.length === 0) continue;
    lines.push("", "## " + title);
    for (const item of items) lines.push("- " + item);
  }
  return lines.join("\n");
}
