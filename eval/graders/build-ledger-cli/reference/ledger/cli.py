"""Command-line entry point: argument parsing, validation and dispatch."""

import argparse
import re
import sys

from ledger import reports
from ledger.amounts import is_commodity
from ledger.journal import JournalUnreadable, load, parse_date

DEPTH_RE = re.compile(r"[1-9][0-9]*")


class UsageError(Exception):
    pass


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="ledger", description="Plain-text double-entry accounting.")
    parser.add_argument("-f", "--file", required=True, help="journal file")
    parser.add_argument("--strict", action="store_true", help="require declared accounts")
    commands = parser.add_subparsers(dest="command", required=True)

    def with_filters(command, dates=True):
        command.add_argument("patterns", nargs="*", metavar="PATTERN")
        if dates:
            command.add_argument("--begin")
            command.add_argument("--end")
        return command

    balance = with_filters(commands.add_parser("balance", help="account balances"))
    balance.add_argument("--depth")
    balance.add_argument("--flat", action="store_true")
    balance.add_argument("--empty", action="store_true")
    balance.add_argument("--value")
    with_filters(commands.add_parser("register", help="posting register")).add_argument(
        "--running", action="store_true")
    with_filters(commands.add_parser("print", help="canonical transactions"))
    with_filters(commands.add_parser("accounts", help="account names"), dates=False)
    for name in ("payees", "prices", "stats", "check"):
        commands.add_parser(name)
    return parser


def option_date(text: str | None):
    if text is None:
        return None
    date = parse_date(text)
    if date is None:
        raise UsageError(f"invalid date: {text}")
    return date


def make_filters(args) -> reports.Filters:
    patterns = []
    for text in getattr(args, "patterns", []):
        try:
            patterns.append(re.compile(text, re.IGNORECASE))
        except re.error:
            raise UsageError(f"invalid pattern: {text}") from None
    return reports.Filters(patterns, option_date(getattr(args, "begin", None)),
                           option_date(getattr(args, "end", None)))


def validate_balance(args) -> None:
    if args.depth is not None and not DEPTH_RE.fullmatch(args.depth):
        raise UsageError(f"invalid depth: {args.depth}")
    if args.value is not None and not is_commodity(args.value):
        raise UsageError(f"invalid commodity: {args.value}")


def run(args, filters: reports.Filters) -> list[str]:
    journal = load(args.file, args.strict)
    if journal.errors:
        if args.command == "check":
            messages = [str(e) for e in journal.errors] + [reports.plural(len(journal.errors), "error")]
        else:
            messages = [str(journal.errors[0])]
        raise JournalFailure(messages)
    if args.command == "balance":
        depth = int(args.depth) if args.depth else None
        return reports.balance(journal, filters, depth, args.flat, args.empty, args.value)
    if args.command == "register":
        return reports.register(journal, filters, args.running)
    if args.command == "print":
        return reports.print_journal(journal, filters)
    if args.command == "accounts":
        return reports.accounts(journal, filters)
    return {"payees": reports.payees, "prices": reports.prices, "stats": reports.stats,
            "check": reports.check}[args.command](journal)


class JournalFailure(Exception):
    def __init__(self, messages: list[str]):
        super().__init__(messages)
        self.messages = messages


def parse_args(argv: list[str] | None):
    """Parse the command line; patterns may come before, between or after options."""
    parser = build_parser()
    args, extra = parser.parse_known_args(argv)
    if extra and (not hasattr(args, "patterns") or any(e.startswith("-") for e in extra)):
        parser.error(f"unrecognized arguments: {' '.join(extra)}")
    if extra:
        args.patterns += extra
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        if args.command == "balance":
            validate_balance(args)
        filters = make_filters(args)
    except UsageError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    try:
        lines = run(args, filters)
    except JournalUnreadable:
        print(f"error: cannot read journal: {args.file}", file=sys.stderr)
        return 1
    except JournalFailure as failure:
        print("\n".join(failure.messages), file=sys.stderr)
        return 1
    if lines:
        print("\n".join(lines))
    return 0
