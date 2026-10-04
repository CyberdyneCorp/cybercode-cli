"""Coupons. A coupon reduces the invoice subtotal before tax is computed."""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from .money import ZERO, round_money


@dataclass(frozen=True)
class Coupon:
    code: str
    percent_off: Decimal | None = None
    amount_off: Decimal | None = None
    currency: str | None = None

    def describe(self) -> str:
        if self.percent_off is not None:
            return f"{self.code} ({self.percent_off.normalize():f}% off)"
        return f"{self.code} ({self.amount_off:f} {self.currency} off)"


def discount_for(coupon: Coupon | None, subtotal: Decimal, currency: str) -> Decimal:
    """Discount (>= 0) that ``coupon`` grants on an invoice with this subtotal.

    No coupon, or a subtotal <= 0 (a net credit), gives no discount. A percent coupon
    gives ``subtotal * percent_off / 100`` rounded to the currency's minor unit. An
    amount coupon gives its amount, capped at the subtotal, and must be in the
    invoice currency.
    """
    if coupon is None or subtotal <= 0:
        return round_money(ZERO, currency)
    if coupon.percent_off is not None:
        return round_money(subtotal * coupon.percent_off / 100, currency)
    if coupon.currency != currency:
        raise ValueError(f"coupon {coupon.code} is in {coupon.currency}, invoice is in {currency}")
    return min(round_money(coupon.amount_off, currency), subtotal)
