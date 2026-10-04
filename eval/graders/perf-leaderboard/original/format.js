/** Plain-text rendering used by the CLI. */

export function formatEntry(entry) {
  return `${entry.rank}. ${entry.player} ${entry.score}`;
}

export function formatEntries(entries) {
  return entries.length === 0 ? "(empty)" : entries.map(formatEntry).join("\n");
}

export function formatCard(card) {
  if (card === null) return "(not ranked)";
  const header = `${card.player}: ${card.score} points, rank ${card.rank} of ${card.total}`;
  return `${header}\n${formatEntries(card.neighbours)}`;
}

export function formatSubmit(result) {
  if (!result.accepted) return `rejected (${result.reason})`;
  const previous = result.previous === null ? "new" : `was ${result.previous}`;
  return `accepted (${previous}), rank ${result.rank}`;
}
