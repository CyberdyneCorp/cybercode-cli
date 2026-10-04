"""Sales tax / VAT per jurisdiction.

A jurisdiction has a rate (a fraction: 0.19 is 19%) and a rounding mode:

* ``invoice``: tax = round(taxable amount * rate) once for the whole invoice;
* ``line``: each line's tax is rounded on its own and the results are summed.
"""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from .money import ZERO, round_money


@dataclass(frozen=True)
class Jurisdiction:
    code: str
    rate: Decimal
    mode: str = "invoice"

    def label(self) -> str:
        percent = (self.rate * 100).normalize()
        return f"{self.code} {percent:f}%"


def compute_tax(jurisdiction: Jurisdiction, line_amounts: list[Decimal], currency: str,
                discount: Decimal = ZERO) -> Decimal:
    """Tax on the given line amounts after subtracting ``discount``.

    In ``line`` mode the discount is taxed like a negative line of its own.
    """
    rate = jurisdiction.rate
    if jurisdiction.mode == "invoice":
        taxable = sum(line_amounts, ZERO) - discount
        return round_money(taxable * rate, currency)
    tax = sum((round_money(amount * rate, currency) for amount in line_amounts), ZERO)
    return round_money(tax - round_money(discount * rate, currency), currency)
