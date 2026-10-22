export const meta = {
  name: "review",
  description: "Multi-angle code review of a diff: read-only reviewers per lens and file group, then an independent verifier for each finding.",
  phases: [{ title: "Group" }, { title: "Review" }, { title: "Verify" }, { title: "Report" }],
  prepare: "review_target",
  args: {
    type: "object",
    properties: {
      target: {
        type: "string",
        description: "What to review: empty for uncommitted changes, a ref range like main..HEAD, a commit, or pr:<number>",
      },
      lenses: {
        type: "array",
        items: { type: "string", enum: ["correctness", "security", "tests", "maintainability"] },
        minItems: 1,
        uniqueItems: true,
        default: ["correctness", "security", "tests", "maintainability"],
      },
      max_findings: { type: "integer", minimum: 1, maximum: 60, default: 30 },
      verify: { type: "boolean", default: true },
    },
    additionalProperties: false,
  },
  budget: { max_agents: 24, max_cost_usd: 2, max_wall_seconds: 1800 },
  estimate: "lenses × diff groups + one verifier per finding (at most max_findings)",
};

// Patch bytes per reviewer group.
const GROUP_BYTES = 48 * 1024;

const LENSES = {
  correctness:
    "Correctness: logic errors, wrong conditions, off-by-one errors, unhandled errors and edge cases, races, resource leaks and broken contracts with callers.",
  security:
    "Security: injection, path traversal, unsafe deserialization, missing authorization or validation, secrets in code or logs, and unsafe defaults.",
  tests:
    "Tests: changed behavior without a test, tests that cannot fail, missing regression tests for fixed bugs, and assertions that check the wrong thing.",
  maintainability:
    "Maintainability: only defects likely to cause bugs later, such as duplicated logic that must change together, misleading names or comments, and dead code paths.",
};

export default async function review() {
  const state = newState();
  const diff = args.diff;
  if (diff === undefined || diff === null || !Array.isArray(diff.files)) {
    throw new Error("review needs the host-resolved diff in args.diff; run it by name so the host prepares it");
  }
  for (const text of diff.limitations || []) limit(state, text);
  const lenses = Array.isArray(args.lenses) && args.lenses.length > 0 ? args.lenses : Object.keys(LENSES);
  const maxFindings = intArg(args.max_findings, 30, 1, 60);
  const verify = args.verify !== false;

  phase("Group");
  const files = diff.files
    .filter((file) => typeof file.patch === "string" && file.patch.length > 0)
    .slice()
    .sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  const groups = chunkBySize(files, (file) => file.patch.length, GROUP_BYTES);
  log({ files: files.length, groups: groups.length, lenses });

  phase("Review");
  const raised = await reviewGroups(state, groups, lenses);
  const { findings: unique, invalid } = dedupFindings(raised.map((item) => item.finding));
  if (invalid > 0) limit(state, invalid + " findings failed validation and were dropped");
  const lensesOf = lensIndex(raised);
  const kept = capped(state, unique, maxFindings, "findings over max_findings were not verified or reported");

  phase("Verify");
  const patches = patchIndex(files);
  const verdicts = verify ? await verifyAll(state, kept, patches) : kept.map(() => false);
  const confirmed = kept.filter((_, index) => !verdicts[index]);
  const refuted = verdicts.filter(Boolean).length;

  phase("Report");
  const counts = lensCounts(lenses, raised, confirmed, lensesOf);
  const summary = confirmed.length + " finding" + (confirmed.length === 1 ? "" : "s") +
    " after " + (verify ? "verification (" + refuted + " refuted)" : "review without verification") +
    " across " + files.length + " file" + (files.length === 1 ? "" : "s");
  return envelope(state, summary, { findings: confirmed, refuted, lenses: counts });
}

// Every (lens, group) reviewer in parallel; each raised finding keeps its lens.
async function reviewGroups(state, groups, lenses) {
  const jobs = [];
  groups.forEach((group, index) => {
    for (const lens of lenses) jobs.push({ lens, group, index });
  });
  const results = await parallel(
    jobs.map((job) => () =>
      guarded(state, "review " + job.lens + " group " + (job.index + 1), () =>
        agent(reviewPrompt(job), {
          agent: "reviewer",
          model: "fast",
          schema: findingsSchema(),
          label: "review " + job.lens + " g" + (job.index + 1),
          phase: "Review",
        }),
      ),
    ),
  );
  if (!state.partial && results.length > 0 && results.every((result) => result === null)) {
    // Nothing was reviewed: a clean result here would pass a CI gate.
    throw new Error("every reviewer failed: " + state.limitations.join("; "));
  }
  const raised = [];
  results.forEach((result, index) => {
    if (result === null) return;
    for (const finding of result.findings || []) raised.push({ lens: jobs[index].lens, finding });
    for (const text of result.limitations || []) limit(state, "reviewer: " + text);
  });
  return raised;
}

function reviewPrompt(job) {
  const patch = job.group.map((file) => file.patch).join("\n");
  return task(
    "review " + job.lens + " group " + (job.index + 1),
    "You are reviewing a code change through one lens only.\n" + LENSES[job.lens] + "\n\n" +
      "Report only real defects in the changed lines or code they break, each with the file path relative to the repository root, the line in the new version, a severity (critical, high, medium or low), a one-sentence summary and a concrete failure scenario. " +
      "You may read files to confirm a finding. Return an empty findings list when you find nothing; never invent issues.\n\n" +
      dataBlock("the diff to review", patch),
  );
}

// Finding key → the lenses that raised it.
function lensIndex(raised) {
  const index = new Map();
  for (const item of raised) {
    const finding = validFinding(item.finding);
    if (finding === null) continue;
    const key = findingKey(finding);
    const lenses = index.get(key) || [];
    if (lenses.indexOf(item.lens) < 0) lenses.push(item.lens);
    index.set(key, lenses);
  }
  return index;
}

function patchIndex(files) {
  const index = new Map();
  for (const file of files) index.set(file.path, file.patch);
  return index;
}

// One independent refuter per finding; `true` when it refuted the finding. A refuter
// that failed or never started keeps its finding.
async function verifyAll(state, findings, patches) {
  const verdicts = await parallel(
    findings.map((finding, index) => () =>
      guarded(state, "verify finding " + (index + 1), () =>
        agent(verifyPrompt(finding, patches.get(finding.file) || ""), {
          agent: "reviewer",
          model: "verify",
          schema: verdictSchema(),
          label: "verify " + (index + 1),
          phase: "Verify",
        }),
      ),
    ),
  );
  return verdicts.map((verdict) => verdict !== null && verdict.refuted === true);
}

function verifyPrompt(finding, patch) {
  return task(
    "verify " + finding.file + ":" + finding.line,
    "Another reviewer reported the finding below. Try to refute it: read the code and decide whether the failure scenario can really happen. " +
      "Set refuted to true only with concrete evidence that it cannot; otherwise set it to false and explain what confirms it.\n\n" +
      dataBlock("the reported finding", JSON.stringify(finding, null, 2)) + "\n\n" +
      dataBlock("the patch of " + finding.file, patch),
  );
}

function lensCounts(lenses, raised, confirmed, lensesOf) {
  const counts = {};
  for (const lens of lenses) counts[lens] = { raised: 0, confirmed: 0 };
  for (const item of raised) {
    if (counts[item.lens] !== undefined && validFinding(item.finding) !== null) counts[item.lens].raised++;
  }
  for (const finding of confirmed) {
    for (const lens of lensesOf.get(findingKey(finding)) || []) {
      if (counts[lens] !== undefined) counts[lens].confirmed++;
    }
  }
  return counts;
}
