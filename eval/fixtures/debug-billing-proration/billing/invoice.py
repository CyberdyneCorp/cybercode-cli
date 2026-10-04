"""Invoice model and totals."""
from __future__ import annotations

from dataclasses import dataclass, field
from datetime import date
from decimal import Decimal

from .discounts import Coupon, discount_for
from .money import round_money, sum_money
from .tax import Jurisdiction, compute_tax


@dataclass(frozen=True)
class Line:
    kind: str          # "plan", "credit" or "charge"
    description: str
    amount: Decimal    # already rounded to the currency's minor unit
    start: date
    end: date          # exclusive


@dataclass
class Invoice:
    customer: str
    currency: str
    lines: list[Line]
    subtotal: Decimal
    discount: Decimal
    tax: Decimal
    total: Decimal
    coupon: Coupon | None = None
    jurisdiction: Jurisdiction | None = None
    notes: list[str] = field(default_factory=list)


def build_invoice(customer: str, currency: str, lines: list[Line], coupon: Coupon | None,
                  jurisdiction: Jurisdiction) -> Invoice:
    """Total the lines: subtotal, coupon discount, tax on the discounted amount, total."""
    for line in lines:
        if round_money(line.amount, currency) != line.amount:
            raise ValueError(f"line {line.description!r} is not rounded to {currency} minor units")
    amounts = [line.amount for line in lines]
    subtotal = sum_money(amounts, currency)
    discount = discount_for(coupon, subtotal, currency)
    tax = compute_tax(jurisdiction, amounts, currency)
    total = round_money(subtotal - discount + tax, currency)
    return Invoice(customer=customer, currency=currency, lines=list(lines), subtotal=subtotal,
                   discount=discount, tax=tax, total=total, coupon=coupon,
                   jurisdiction=jurisdiction)
