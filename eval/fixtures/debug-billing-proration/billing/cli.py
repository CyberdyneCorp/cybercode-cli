"""Command line entry point: ``python3 -m billing ...``."""
from __future__ import annotations

import argparse
import sys
from datetime import date

from .catalog import DEFAULT_CATALOG, Catalog
from .export import invoices_to_jsonl
from .render import render_invoice
from .schedule import upcoming_periods
from .service import BillingService


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="billing", description="Render subscription invoices.")
    parser.add_argument("--catalog", default=str(DEFAULT_CATALOG), help="pricing catalog JSON")
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("period", "change"):
        cmd = sub.add_parser(name)
        cmd.add_argument("--customer", default="customer")
        cmd.add_argument("--plan", required=True)
        cmd.add_argument("--anchor", required=True, type=date.fromisoformat)
        cmd.add_argument("--coupon")
        cmd.add_argument("--jurisdiction", default="NONE")
    sub.choices["period"].add_argument("--index", type=int, default=0)
    sub.choices["change"].add_argument("--to", required=True, dest="new_plan")
    sub.choices["change"].add_argument("--on", required=True, type=date.fromisoformat)
    for name in ("period", "change"):
        sub.choices[name].add_argument("--json", action="store_true", help="print JSON instead of text")
    schedule = sub.add_parser("schedule", help="list upcoming billing periods of a plan")
    schedule.add_argument("--plan", required=True)
    schedule.add_argument("--anchor", required=True, type=date.fromisoformat)
    schedule.add_argument("--count", type=int, default=12)
    return parser


def _schedule(catalog: Catalog, args) -> str:
    plan = catalog.plan(args.plan)
    rows = upcoming_periods(args.anchor, plan.interval_months, args.count)
    return "".join(f"{k}\t{start.isoformat()}\t{end.isoformat()}\n" for k, (start, end) in enumerate(rows))


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        catalog = Catalog.load(args.catalog)
        if args.command == "schedule":
            sys.stdout.write(_schedule(catalog, args))
            return 0
        service = BillingService(catalog)
        sub = service.subscribe(args.customer, args.plan, args.anchor, args.coupon, args.jurisdiction)
        if args.command == "period":
            invoice = service.period_invoice(sub, args.index)
        else:
            invoice = service.change_plan(sub, args.new_plan, args.on)
    except (KeyError, ValueError) as exc:
        print(f"billing: {exc}", file=sys.stderr)
        return 1
    sys.stdout.write(invoices_to_jsonl([invoice]) if args.json else render_invoice(invoice))
    return 0
