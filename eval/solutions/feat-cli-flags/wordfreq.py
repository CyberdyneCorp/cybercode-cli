"""Print word frequencies of a text file."""

import argparse
import re
import sys
from collections import Counter
from pathlib import Path

WORD = re.compile(r"[A-Za-z0-9']+")


def positive_int(value: str) -> int:
    try:
        number = int(value)
    except ValueError:
        raise argparse.ArgumentTypeError(f"not an integer: {value!r}") from None
    if number < 1:
        raise argparse.ArgumentTypeError(f"must be at least 1: {value!r}")
    return number


def count_words(text: str, ignore_case: bool = False, min_length: int = 1) -> Counter:
    words = WORD.findall(text.lower() if ignore_case else text)
    return Counter(word for word in words if len(word) >= min_length)


def format_counts(counts: Counter, top: int | None = None) -> str:
    ranked = sorted(counts.items(), key=lambda item: (-item[1], item[0]))
    return "".join(f"{count}\t{word}\n" for word, count in ranked[:top])


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Print word frequencies of a text file.")
    parser.add_argument("path", help="UTF-8 text file to read")
    parser.add_argument("--top", type=positive_int, default=10, metavar="N",
                        help="print only the N most frequent words (default 10)")
    parser.add_argument("--ignore-case", action="store_true",
                        help="count words case-insensitively (output in lowercase)")
    parser.add_argument("--min-length", type=positive_int, default=1, metavar="N",
                        help="skip words shorter than N characters (default 1)")
    args = parser.parse_args(argv)
    try:
        text = Path(args.path).read_text(encoding="utf-8")
    except OSError as error:
        print(f"wordfreq: cannot read {args.path}: {error.strerror}", file=sys.stderr)
        return 1
    counts = count_words(text, ignore_case=args.ignore_case, min_length=args.min_length)
    sys.stdout.write(format_counts(counts, top=args.top))
    return 0


if __name__ == "__main__":
    sys.exit(main())
