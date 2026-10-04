"""Amount parsing, balances and display formatting."""

import re
from dataclasses import dataclass
from decimal import ROUND_HALF_UP, Decimal

NUMBER_RE = re.compile(r"-?(?:[0-9]{1,3}(?:,[0-9]{3})+|[0-9]+)(?:\.([0-9]+))?")
COMMODITY_RE = re.compile(r"[A-Z]+")


def parse_number(text: str) -> tuple[Decimal, int] | None:
    """Return (value, decimals written) for a NUMBER token, or None if it is not one."""
    match = NUMBER_RE.fullmatch(text)
    if not match:
        return None
    return Decimal(text.replace(",", "")), len(match.group(1) or "")


def is_commodity(text: str) -> bool:
    return COMMODITY_RE.fullmatch(text) is not None


@dataclass(frozen=True)
class Amount:
    quantity: Decimal
    commodity: str


class Balance:
    """A sum of amounts, kept exactly per commodity."""

    def __init__(self):
        self.totals: dict[str, Decimal] = {}

    def add(self, amount: Amount) -> None:
        self.totals[amount.commodity] = self.totals.get(amount.commodity, Decimal(0)) + amount.quantity

    def add_balance(self, other: "Balance") -> None:
        for commodity, quantity in other.totals.items():
            self.add(Amount(quantity, commodity))

    def nonzero(self) -> list[Amount]:
        return [Amount(q, c) for c, q in sorted(self.totals.items()) if q != 0]

    def is_zero(self) -> bool:
        return not self.nonzero()


class Formatter:
    """Formats amounts with each commodity's display precision."""

    def __init__(self, precision: dict[str, int]):
        self.precision = precision

    def amount(self, amount: Amount) -> str:
        places = self.precision.get(amount.commodity, 0)
        value = amount.quantity.quantize(Decimal(1).scaleb(-places), rounding=ROUND_HALF_UP)
        if value == 0:
            value = value.copy_abs()
        return f"{value:,.{places}f} {amount.commodity}"

    def balance(self, balance: Balance) -> list[str]:
        return [self.amount(a) for a in balance.nonzero()] or ["0"]
