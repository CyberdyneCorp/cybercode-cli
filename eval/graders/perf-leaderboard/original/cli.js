#!/usr/bin/env node
/**
 * Replay a command script against a fresh LeaderboardService and print one result per command.
 *
 *   node src/cli.js [script.txt]      (reads stdin when no file is given)
 *
 * Commands (one per line, blank lines and lines starting with # are skipped):
 *   submit P S | correct P S | ban P | unban P | rank P | score P | top K | around P N
 *   card P | range LO HI | pct P | size | close | seasons
 */
import { readFileSync, realpathSync } from "node:fs";

import { formatCard, formatEntries, formatSubmit } from "./format.js";
import { LeaderboardService } from "./service.js";

const integer = (text) => (/^-?\d+$/.test(text) ? Number(text) : Number.NaN);
const number = (text) => (text === undefined ? Number.NaN : Number(text));

const COMMANDS = {
  submit: (svc, [p, s]) => formatSubmit(svc.submit(p, integer(s))),
  correct: (svc, [p, s]) => `previous ${svc.correct(p, integer(s))}`,
  ban: (svc, [p]) => (svc.ban(p) ? "banned (removed)" : "banned"),
  unban: (svc, [p]) => (svc.unban(p) ? "unbanned" : "not banned"),
  rank: (svc, [p]) => String(svc.board.rank(p)),
  score: (svc, [p]) => String(svc.board.score(p)),
  top: (svc, [k]) => formatEntries(svc.board.topK(integer(k))),
  around: (svc, [p, n]) => {
    const entries = svc.board.around(p, integer(n));
    return entries === null ? "(not ranked)" : formatEntries(entries);
  },
  card: (svc, [p]) => formatCard(svc.card(p)),
  range: (svc, [lo, hi]) => String(svc.bracket(number(lo), number(hi))),
  pct: (svc, [p]) => String(svc.cutoff(number(p))),
  size: (svc) => String(svc.board.size),
  close: (svc) => `season closed\n${formatEntries(svc.closeSeason())}`,
  seasons: (svc) => svc.seasons().map((s) => `season ${s.season}: ${s.podium.map((e) => e.player).join(", ") || "-"}`).join("\n") || "(none)",
};

export function run(script) {
  const service = new LeaderboardService();
  const out = [];
  for (const raw of script.split("\n")) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    const [name, ...args] = line.split(/\s+/);
    const command = COMMANDS[name];
    if (!command) {
      out.push(`error: unknown command ${name}`);
      continue;
    }
    try {
      out.push(command(service, args));
    } catch (error) {
      out.push(`error: ${error.message}`);
    }
  }
  return out.join("\n");
}

if (process.argv[1] && import.meta.filename === realpathSync(process.argv[1])) {
  const script = readFileSync(process.argv[2] ?? 0, "utf8");
  process.stdout.write(`${run(script)}\n`);
}
