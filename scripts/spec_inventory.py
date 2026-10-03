"""Refresh ROADMAP.md counts from the first phase tag of each requirement."""

from pathlib import Path
import re


def count_phases(path):
    counts = [0] * 5
    requirements = re.split(r"^### Requirement:", path.read_text(), flags=re.M)[1:]
    for requirement in requirements:
        phase = re.search(r"\(P([0-4])\)", requirement)
        if phase is None:
            raise ValueError(f"Missing phase in {path}: {requirement.splitlines()[0]}")
        counts[int(phase[1])] += 1
    return counts


def main():
    root = Path(__file__).resolve().parents[1]
    specs = sorted((root / "openspec/specs").glob("*/spec.md"))
    totals = [0] * 5
    rows = [
        "| Capability | P0 | P1 | P2 | P3 | P4 | Total |",
        "|---|--:|--:|--:|--:|--:|--:|",
    ]
    for spec in specs:
        counts = count_phases(spec)
        totals = [a + b for a, b in zip(totals, counts)]
        cells = [str(count) if count else "" for count in counts]
        rows.append(f"| `{spec.parent.name}` | " + " | ".join(cells) + f" | {sum(counts)} |")
    rows.append("| **Total** | " + " | ".join(f"**{n}**" for n in totals) + f" | **{sum(totals)}** |")
    roadmap = root / "ROADMAP.md"
    text = re.sub(
        r"\| Capability \| P0.*?\| \*\*Total\*\*[^\n]*",
        lambda _: "\n".join(rows),
        roadmap.read_text(),
        flags=re.S,
    )
    text = re.sub(r"\d+ capabilities, 0 untagged", f"{len(specs)} capabilities, 0 untagged", text)
    roadmap.write_text(text)
    print(f"{len(specs)} capabilities, {sum(totals)} requirements; P0–P4: {totals}")


if __name__ == "__main__":
    main()
