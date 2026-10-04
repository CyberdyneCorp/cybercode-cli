"""Command line: python3 -m analytics EVENTS.csv [--gap SECONDS] [--top K]"""

import argparse
import sys

from .events import parse_events
from .report import build_report


def positive_int(text: str) -> int:
    if not text.isdigit() or int(text) < 1:
        raise argparse.ArgumentTypeError(f"expected a positive integer, got {text!r}")
    return int(text)


def non_negative_int(text: str) -> int:
    if not text.isdigit():
        raise argparse.ArgumentTypeError(f"expected a non-negative integer, got {text!r}")
    return int(text)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="analytics", description="Clickstream session report.")
    parser.add_argument("events", help="CSV file with a user_id,timestamp,page header")
    parser.add_argument("--gap", type=non_negative_int, default=1800, help="session gap in seconds")
    parser.add_argument("--top", type=positive_int, default=10, help="number of top users")
    args = parser.parse_args(argv)
    try:
        with open(args.events, encoding="utf-8", newline="") as handle:
            events = parse_events(handle)
    except OSError as error:
        print(f"error: cannot read {args.events}: {error.strerror}", file=sys.stderr)
        return 1
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    sys.stdout.write(build_report(events, args.gap, args.top))
    return 0


if __name__ == "__main__":
    sys.exit(main())
