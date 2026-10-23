export const meta = {
  name: "research",
  description: "Cross-checked research: searchers per source and angle, readers that extract claims with locations, an independent cross-check per claim, then an answer that cites only verified claims.",
  phases: [{ title: "Sweep" }, { title: "Read" }, { title: "Cross-check" }, { title: "Synthesize" }],
  prepare: "research_sources",
  args: {
    type: "object",
    properties: {
      question: { type: "string", minLength: 1 },
      sources: {
        type: "array",
        items: { type: "string", enum: ["web", "repo", "docs"] },
        minItems: 1,
        uniqueItems: true,
        default: ["repo", "docs"],
      },
      angles: { type: "integer", minimum: 1, maximum: 6, default: 3 },
      max_sources: { type: "integer", minimum: 1, maximum: 40, default: 12 },
    },
    required: ["question"],
    additionalProperties: false,
  },
  budget: { max_agents: 40, max_cost_usd: 5, max_wall_seconds: 3600 },
  estimate: "sources × angles searchers + one reader per source (at most max_sources) + one cross-check per claim (at most 30)",
};

// Claims cross-checked at most; the rest are reported as not checked.
const MAX_CLAIMS = 30;

const ANGLES = [
  "the most direct answer to the question",
  "how it is implemented, configured or used in practice",
  "limitations, caveats, failure cases and counter-examples",
  "history, rationale and the decisions behind it",
  "alternatives and how they compare",
  "concrete examples and their outcomes",
];

const MODALITIES = {
  repo: "the code of this repository (read, glob and grep it)",
  docs: "the documentation in this repository: READMEs, docs folders, specifications and comments",
  web: "the web (use web search and fetch pages)",
};

export default async function research() {
  const state = newState();
  const sources = requestedSources(state);
  const angles = ANGLES.slice(0, intArg(args.angles, 3, 1, 6));

  phase("Sweep");
  const candidates = await sweep(state, sources, angles);
  const kept = capped(state, rankSources(candidates), intArg(args.max_sources, 12, 1, 40), "sources over max_sources were not read");

  phase("Read");
  const claims = await readAll(state, kept);
  const checked = capped(state, claims, MAX_CLAIMS, "claims over 30 were not cross-checked");

  phase("Cross-check");
  const verdicts = await crossCheck(state, checked, kept);
  const graded = checked.map((claim, index) => grade(claim, verdicts[index]));

  phase("Synthesize");
  const verified = graded.filter((claim) => claim.verified);
  const unverified = graded.filter((claim) => !claim.verified).map((claim) => claim.text);
  const answer = await synthesize(state, verified, unverified);
  const summary = verified.length + " verified and " + unverified.length + " unverified claims from " + kept.length + " sources";
  return envelope(state, summary, {
    answer,
    claims: graded,
    sources: kept.map((source) => ({ ref: source.ref, kind: source.kind, title: source.title })),
    unverified,
  });
}

// The requested sources; `web` only when the host found the web tools permitted.
function requestedSources(state) {
  const requested = Array.isArray(args.sources) && args.sources.length > 0 ? args.sources : ["repo", "docs"];
  return requested.filter((source) => {
    if (source !== "web" || args.web_permitted === true) return true;
    limit(state, "web sources were not permitted (websearch or webfetch is not allowed for the reviewer agent here)");
    return false;
  });
}

function candidateSchema() {
  return {
    type: "object",
    properties: {
      sources: {
        type: "array",
        items: {
          type: "object",
          properties: {
            ref: { type: "string", minLength: 1, description: "A URL, or a repository-relative path" },
            title: { type: "string" },
            why: { type: "string" },
          },
          required: ["ref", "title", "why"],
          additionalProperties: false,
        },
      },
    },
    required: ["sources"],
    additionalProperties: false,
  };
}

// One searcher per (source, angle).
async function sweep(state, sources, angles) {
  const jobs = [];
  for (const source of sources) angles.forEach((angle, index) => jobs.push({ source, angle, index }));
  const results = await parallel(
    jobs.map((job) => () =>
      guarded(state, "search " + job.source + " angle " + (job.index + 1), () =>
        agent(
          task(
            "search " + job.source + " angle " + (job.index + 1),
            "Find the best sources to answer the question below, searching " + MODALITIES[job.source] +
              ", from this angle: " + job.angle + ". Return up to 6 sources with why each one matters. Do not answer the question.\n\n" +
              dataBlock("the research question", args.question),
          ),
          { agent: "reviewer", model: "fast", schema: candidateSchema(), label: "search " + job.source + " a" + (job.index + 1), phase: "Sweep" },
        ),
      ),
    ),
  );
  const candidates = [];
  results.forEach((result, index) => {
    if (result === null) return;
    for (const source of result.sources || []) candidates.push({ ref: source.ref, title: source.title, kind: jobs[index].source });
  });
  return candidates;
}

function sourceKey(ref) {
  const text = String(ref).trim();
  return /^https?:\/\//i.test(text) ? text.toLowerCase().replace(/\/+$/, "").replace(/#.*$/, "") : text.replace(/^\.\//, "");
}

// Unique sources, most often proposed first, then by reference.
function rankSources(candidates) {
  const byKey = new Map();
  for (const candidate of candidates) {
    const key = sourceKey(candidate.ref);
    if (key === "") continue;
    const known = byKey.get(key);
    if (known === undefined) byKey.set(key, { ref: key, title: candidate.title, kind: candidate.kind, votes: 1 });
    else known.votes++;
  }
  return Array.from(byKey.values())
    .sort((a, b) => b.votes - a.votes || (a.ref < b.ref ? -1 : a.ref > b.ref ? 1 : 0))
    .map((source) => ({ ref: source.ref, title: source.title, kind: source.kind }));
}

function claimSchema() {
  return {
    type: "object",
    properties: {
      claims: {
        type: "array",
        items: {
          type: "object",
          properties: {
            text: { type: "string", minLength: 1 },
            location: { type: "string", description: "Where in the source: a line, section or quote" },
          },
          required: ["text", "location"],
          additionalProperties: false,
        },
      },
    },
    required: ["claims"],
    additionalProperties: false,
  };
}

// One reader per source extracts the claims relevant to the question.
async function readAll(state, sources) {
  const results = await parallel(
    sources.map((source, index) => () =>
      guarded(state, "read " + source.ref, () =>
        agent(
          task(
            "read source " + (index + 1),
            "Read the source below and extract up to 8 factual claims relevant to the question, each with where it appears in the source. Quote rather than interpret.\n\n" +
              dataBlock("the research question", args.question) + "\n\n" + dataBlock("the source to read", source.ref),
          ),
          { agent: "reviewer", model: "fast", schema: claimSchema(), label: "read s" + (index + 1), phase: "Read" },
        ),
      ),
    ),
  );
  const claims = [];
  results.forEach((result, index) => {
    if (result === null) return;
    for (const claim of result.claims || []) claims.push({ text: claim.text, source: sources[index].ref, location: claim.location });
  });
  return claims;
}

function checkSchema() {
  return {
    type: "object",
    properties: {
      supported_by: { type: "array", items: { type: "string" } },
      contradicted_by: { type: "array", items: { type: "string" } },
      confidence: { type: "string", enum: ["low", "medium", "high"] },
    },
    required: ["supported_by", "contradicted_by", "confidence"],
    additionalProperties: false,
  };
}

// An independent agent checks each claim against the other sources.
async function crossCheck(state, claims, sources) {
  return parallel(
    claims.map((claim, index) => () => {
      const others = sources.filter((source) => source.ref !== claim.source).map((source) => source.ref);
      if (others.length === 0) return Promise.resolve(null);
      return guarded(state, "cross-check claim " + (index + 1), () =>
        agent(
          task(
            "cross-check claim " + (index + 1),
            "Check the claim below against the other sources listed. Name the sources that independently support it and those that contradict it; read them as needed.\n\n" +
              dataBlock("the claim", claim.text) + "\n\n" + dataBlock("the other sources, one per line", others.join("\n")),
          ),
          { agent: "reviewer", model: "verify", schema: checkSchema(), label: "check c" + (index + 1), phase: "Cross-check" },
        ),
      );
    }),
  );
}

// A claim is verified when another source supports it and none contradicts it.
function grade(claim, verdict) {
  const sources = [claim.source];
  let confidence = "low";
  let contradicted = false;
  if (verdict !== null) {
    for (const ref of verdict.supported_by || []) {
      const key = sourceKey(ref);
      if (key !== "" && sources.indexOf(key) < 0) sources.push(key);
    }
    contradicted = (verdict.contradicted_by || []).length > 0;
    confidence = verdict.confidence;
  }
  return { text: claim.text, sources, location: claim.location, verified: sources.length > 1 && !contradicted, confidence };
}

async function synthesize(state, verified, unverified) {
  if (verified.length === 0 && unverified.length === 0) return "";
  const listed = verified.map((claim, index) => "[" + (index + 1) + "] " + claim.text + " (" + claim.sources.join(", ") + ")").join("\n");
  const result = await guarded(state, "synthesize", () =>
    agent(
      task(
        "synthesize answer",
        "Answer the question using only the verified claims, citing them as [n]. Then add a section titled Unverified listing the unverified claims, clearly marked as not confirmed.\n\n" +
          dataBlock("the research question", args.question) + "\n\n" + dataBlock("the verified claims", listed) + "\n\n" +
          dataBlock("the unverified claims, one per line", unverified.join("\n")),
      ),
      { agent: "reviewer", model: "smart", label: "synthesize", phase: "Synthesize" },
    ),
  );
  return result === null ? "" : result.text;
}
