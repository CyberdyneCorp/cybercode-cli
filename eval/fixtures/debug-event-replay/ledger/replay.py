"""Operator tool: replay the event log and print the resulting balances.

    python3 -m ledger.replay DATA_DIR [--from-scratch | --verify]
"""

from __future__ import annotations

import argparse
import sys
from decimal import Decimal

from .money import format_amount
from .service import WalletService


def balances(data_dir: str, use_snapshot: bool) -> tuple[dict[str, Decimal], int]:
    service = WalletService(data_dir, use_snapshot=use_snapshot)
    return {w: service.balance(w) for w in service.wallets()}, service.last_seq


def render(state: dict[str, Decimal], last_seq: int) -> list[str]:
    lines = [f"{wallet_id} {format_amount(balance)}" for wallet_id, balance in state.items()]
    return lines + [f"last_seq {last_seq}"]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python3 -m ledger.replay", description=__doc__)
    parser.add_argument("data_dir")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--from-scratch", action="store_true", help="ignore the snapshot")
    mode.add_argument("--verify", action="store_true", help="compare snapshot+tail with a full replay")
    args = parser.parse_args(argv)

    if args.verify:
        from_snapshot = balances(args.data_dir, use_snapshot=True)
        from_scratch = balances(args.data_dir, use_snapshot=False)
        print("\n".join(render(*from_snapshot)))
        if from_snapshot != from_scratch:
            print("replay mismatch: snapshot+tail differs from a full replay", file=sys.stderr)
            return 1
        return 0
    print("\n".join(render(*balances(args.data_dir, use_snapshot=not args.from_scratch))))
    return 0


if __name__ == "__main__":
    sys.exit(main())
