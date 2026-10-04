"""Billing facade used by the API layer and the CLI."""
from __future__ import annotations

from dataclasses import dataclass
from datetime import date

from .catalog import Catalog
from .invoice import Invoice, Line, build_invoice
from .money import round_money
from .periods import period_bounds, period_index
from .proration import plan_change_amounts


@dataclass
class Subscription:
    customer: str
    plan_code: str
    anchor: date
    coupon_code: str | None = None
    jurisdiction_code: str = "NONE"


class BillingService:
    def __init__(self, catalog: Catalog | None = None):
        self.catalog = catalog if catalog is not None else Catalog.load()

    def subscribe(self, customer: str, plan_code: str, anchor: date, coupon: str | None = None,
                  jurisdiction: str = "NONE") -> Subscription:
        self.catalog.plan(plan_code)
        self.catalog.coupon(coupon)
        self.catalog.jurisdiction(jurisdiction)
        return Subscription(customer, plan_code, anchor, coupon, jurisdiction)

    def _finish(self, sub: Subscription, currency: str, lines: list[Line]) -> Invoice:
        return build_invoice(sub.customer, currency, lines, self.catalog.coupon(sub.coupon_code),
                             self.catalog.jurisdiction(sub.jurisdiction_code))

    def period_invoice(self, sub: Subscription, index: int) -> Invoice:
        """Invoice for billing period ``index`` (0-based) on the subscription's current plan."""
        plan = self.catalog.plan(sub.plan_code)
        start, end = period_bounds(sub.anchor, plan.interval_months, index)
        line = Line("plan", plan.name, round_money(plan.price, plan.currency), start, end)
        return self._finish(sub, plan.currency, [line])

    def change_plan(self, sub: Subscription, new_plan_code: str, on: date) -> Invoice:
        """Switch the subscription to another plan on ``on`` and return the proration invoice."""
        old = self.catalog.plan(sub.plan_code)
        new = self.catalog.plan(new_plan_code)
        if new.code == old.code:
            raise ValueError("subscription is already on this plan")
        if new.currency != old.currency or new.interval_months != old.interval_months:
            raise ValueError("plan changes must keep the currency and billing interval")
        index = period_index(sub.anchor, old.interval_months, on)
        start, end = period_bounds(sub.anchor, old.interval_months, index)
        credit, charge = plan_change_amounts(old.price, new.price, old.currency, start, end, on)
        lines = [
            Line("credit", f"Unused time on {old.name}", credit, on, end),
            Line("charge", f"Remaining time on {new.name}", charge, on, end),
        ]
        sub.plan_code = new.code
        return self._finish(sub, old.currency, lines)
