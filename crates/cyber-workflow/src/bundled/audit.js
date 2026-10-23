export const meta = {
  name: "audit",
  description: "Codebase-wide bug or security sweep: read-only finders per chunk of files, then independent refuters for every finding (a majority vote for critical and high).",
  phases: [{ title: "Scan" }, { title: "Find" }, { title: "Verify" }, { title: "Report" }],
  tools: ["glob"],
  args: {
    type: "object",
    properties: {
      scope: { type: "string", minLength: 1, default: "**/*", description: "Glob of the files to audit, relative to the Location" },
      exclude: { type: "array", items: { type: "string", minLength: 1 }, default: [] },
      focus: { type: "string", enum: ["bugs", "security", "both"], default: "both" },
      max_files: { type: "integer", minimum: 1, maximum: 2000, default: 400 },
      chunk_files: { type: "integer", minimum: 1, maximum: 50, default: 12 },
      votes: { type: "integer", minimum: 1, maximum: 7, default: 3 },
    },
    additionalProperties: false,
  },
  budget: { max_agents: 120, max_cost_usd: 10, max_wall_seconds: 7200 },
  estimate: "one finder per chunk of files + votes refuters per critical or high finding + one per other finding",
};

// Files larger than this are listed as skipped rather than given to a finder.
const MAX_FILE_BYTES = 256 * 1024;
// The most paths one glob returns.
const GLOB_MAX = 10000;

const CHECKLISTS = {
  bugs:
    "Bugs: logic errors, unhandled errors, wrong edge cases, races and deadlocks, resource leaks, incorrect error propagation and broken invariants.",
  security:
    "Security: injection (SQL, shell, template), path traversal, unsafe deserialization, missing authentication or authorization, secrets in code or logs, weak cryptography and unsafe defaults.",
};

export default async function audit() {
  const state = newState();
  const focus = args.focus === "bugs" || args.focus === "security" ? args.focus : "both";
  const votes = intArg(args.votes, 3, 1, 7);

  phase("Scan");
  const scan = await scanFiles(state);
  const chunks = chunkByDirectory(scan.files, intArg(args.chunk_files, 12, 1, 50));
  log({ files: scan.files.length, skipped: scan.skipped.length, chunks: chunks.length });

  phase("Find");
  const found = await findAll(state, chunks, focus);
  const { findings: unique, invalid } = dedupFindings(found.findings);
  if (invalid > 0) limit(state, invalid + " findings failed validation and were dropped");

  phase("Verify");
  const verdicts = await verifyAll(state, unique, votes);
  const findings = unique.filter((_, index) => !verdicts[index]);
  const refuted = verdicts.filter(Boolean).length;

  phase("Report");
  const skipped = scan.skipped.concat(found.unscanned);
  const coverage = { files_scanned: found.scanned, files_skipped: skipped, chunks: chunks.length };
  const summary = findings.length + " finding" + (findings.length === 1 ? "" : "s") + " (" + refuted +
    " refuted) in " + found.scanned + " of " + (found.scanned + skipped.length) + " files";
  return envelope(state, summary, { findings, refuted, coverage });
}

// The files in scope, sorted: excluded files are dropped, files over 256 KiB and files past
// `max_files` are skipped with their reason.
async function scanFiles(state) {
  const scope = typeof args.scope === "string" && args.scope.length > 0 ? args.scope : "**/*";
  const listed = await tools.glob({ pattern: scope, max: GLOB_MAX, sizes: true });
  if (listed.truncated) limit(state, "the scope matched more than " + GLOB_MAX + " files; only the first " + GLOB_MAX + " were considered");
  const exclude = Array.isArray(args.exclude) ? args.exclude : [];
  const files = [];
  const skipped = [];
  listed.paths.forEach((path, index) => {
    if (exclude.some((pattern) => globMatch(pattern, path))) return;
    const size = Array.isArray(listed.sizes) ? listed.sizes[index] : 0;
    if (size > MAX_FILE_BYTES) skipped.push({ path, reason: "larger than 256 KiB" });
    else files.push(path);
  });
  files.sort();
  const maxFiles = intArg(args.max_files, 400, 1, 2000);
  for (const path of files.slice(maxFiles)) skipped.push({ path, reason: "max_files" });
  if (files.length > maxFiles) {
    log({ dropped: files.length - maxFiles, reason: "max_files" });
    limit(state, (files.length - maxFiles) + " files skipped by max_files (" + maxFiles + ")");
  }
  if (skipped.some((item) => item.reason !== "max_files")) {
    limit(state, skipped.filter((item) => item.reason !== "max_files").length + " files larger than 256 KiB were skipped");
  }
  return { files: files.slice(0, maxFiles), skipped };
}

// One read-only finder per chunk. Files of a chunk whose finder did not finish are
// reported as not scanned.
async function findAll(state, chunks, focus) {
  const results = await parallel(
    chunks.map((chunk, index) => () =>
      guarded(state, "find chunk " + (index + 1), () =>
        agent(findPrompt(chunk, index, focus), {
          agent: "reviewer",
          model: "fast",
          schema: findingsSchema(),
          label: "find c" + (index + 1),
          phase: "Find",
        }),
      ),
    ),
  );
  const findings = [];
  const unscanned = [];
  let scanned = 0;
  results.forEach((result, index) => {
    if (result === null) {
      for (const path of chunks[index]) unscanned.push({ path, reason: state.partial ? "budget" : "finder failed" });
      return;
    }
    scanned += chunks[index].length;
    for (const finding of result.findings || []) findings.push(finding);
    for (const text of result.limitations || []) limit(state, "finder: " + text);
  });
  return { findings, scanned, unscanned };
}

function findPrompt(chunk, index, focus) {
  const checklist = focus === "both" ? CHECKLISTS.bugs + "\n" + CHECKLISTS.security : CHECKLISTS[focus];
  return task(
    "find chunk " + (index + 1),
    "Audit the files listed below. Read each one and report real defects only.\n" + checklist + "\n\n" +
      "Each finding needs the file path relative to the repository root, its line, a severity (critical, high, medium or low), a one-sentence summary and a concrete failure scenario. " +
      "Return an empty findings list when you find nothing; never invent issues.\n\n" +
      dataBlock("the files to audit, one path per line", chunk.join("\n")),
  );
}

// Critical and high findings get `votes` refuters and are dropped when a majority refutes
// them; the others get one. `true` marks a refuted finding.
async function verifyAll(state, findings, votes) {
  const jobs = [];
  findings.forEach((finding, index) => {
    const count = severityRank(finding.severity) <= 1 ? votes : 1;
    for (let vote = 0; vote < count; vote++) jobs.push({ index, vote, finding });
  });
  const verdicts = await parallel(
    jobs.map((job) => () =>
      guarded(state, "verify finding " + (job.index + 1), () =>
        agent(refutePrompt(job.finding, job.vote), {
          agent: "reviewer",
          model: "verify",
          schema: verdictSchema(),
          label: "verify " + (job.index + 1) + "." + (job.vote + 1),
          phase: "Verify",
        }),
      ),
    ),
  );
  const refutes = findings.map(() => 0);
  const asked = findings.map(() => 0);
  jobs.forEach((job, index) => {
    asked[job.index]++;
    if (verdicts[index] !== null && verdicts[index].refuted === true) refutes[job.index]++;
  });
  return findings.map((_, index) => refutes[index] * 2 > asked[index]);
}

function refutePrompt(finding, vote) {
  return task(
    "refute " + finding.file + ":" + finding.line + " #" + (vote + 1),
    "An auditor reported the finding below. Independently try to refute it: read the code and decide whether the failure scenario can really happen. " +
      "Set refuted to true only with concrete evidence that it cannot; otherwise set it to false and explain what confirms it.\n\n" +
      dataBlock("the reported finding", JSON.stringify(finding, null, 2)),
  );
}
