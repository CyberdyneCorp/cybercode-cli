"""Print word frequencies of a text file."""

import argparse
import re
import sys
from collections import Counter
from pathlib import Path

WORD = re.compile(r"[A-Za-z0-9']+")


def count_words(text: str) -> Counter:
    return Counter(WORD.findall(text))


def format_counts(counts: Counter) -> str:
    ranked = sorted(counts.items(), key=lambda item: (-item[1], item[0]))
    return "".join(f"{count}\t{word}\n" for word, count in ranked)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Print word frequencies of a text file.")
    parser.add_argument("path", help="UTF-8 text file to read")
    args = parser.parse_args(argv)
    try:
        text = Path(args.path).read_text(encoding="utf-8")
    except OSError as error:
        print(f"wordfreq: cannot read {args.path}: {error.strerror}", file=sys.stderr)
        return 1
    sys.stdout.write(format_counts(count_words(text)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
