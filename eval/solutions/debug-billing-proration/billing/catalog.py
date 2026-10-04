"""Plans, coupons and tax jurisdictions, loaded from a JSON pricing export."""
from __future__ import annotations

import json
from dataclasses import dataclass
from decimal import Decimal
from pathlib import Path

from .discounts import Coupon
from .money import normalize_currency
from .tax import Jurisdiction

DEFAULT_CATALOG = Path(__file__).with_name("catalog.json")


@dataclass(frozen=True)
class Plan:
    code: str
    name: str
    price: Decimal
    currency: str
    interval_months: int


def to_decimal(value) -> Decimal:
    """Convert a catalog number to Decimal.

    Strings and ints are exact. A float stands for the decimal number it prints as
    (its shortest repr), so 0.0725 means Decimal("0.0725").
    """
    if isinstance(value, bool):
        raise ValueError(f"not a number: {value!r}")
    if isinstance(value, float):
        return Decimal(repr(value))
    if isinstance(value, (int, str, Decimal)):
        return Decimal(value)
    raise ValueError(f"not a number: {value!r}")


class Catalog:
    def __init__(self, plans: dict[str, Plan], coupons: dict[str, Coupon],
                 jurisdictions: dict[str, Jurisdiction]):
        self.plans = plans
        self.coupons = coupons
        self.jurisdictions = jurisdictions

    @classmethod
    def from_dict(cls, data: dict) -> "Catalog":
        plans = {}
        for code, raw in data.get("plans", {}).items():
            interval = int(raw.get("interval_months", 1))
            if interval not in (1, 3, 12):
                raise ValueError(f"plan {code}: unsupported interval {interval}")
            plans[code] = Plan(code=code, name=raw["name"], price=to_decimal(raw["price"]),
                               currency=normalize_currency(raw["currency"]), interval_months=interval)
        coupons = {}
        for code, raw in data.get("coupons", {}).items():
            if ("percent_off" in raw) == ("amount_off" in raw):
                raise ValueError(f"coupon {code}: needs exactly one of percent_off, amount_off")
            if "percent_off" in raw:
                coupons[code] = Coupon(code=code, percent_off=to_decimal(raw["percent_off"]))
            else:
                coupons[code] = Coupon(code=code, amount_off=to_decimal(raw["amount_off"]),
                                       currency=normalize_currency(raw["currency"]))
        jurisdictions = {}
        for code, raw in data.get("jurisdictions", {}).items():
            mode = raw.get("mode", "invoice")
            if mode not in ("invoice", "line"):
                raise ValueError(f"jurisdiction {code}: unknown mode {mode}")
            jurisdictions[code] = Jurisdiction(code=code, rate=to_decimal(raw["rate"]), mode=mode)
        return cls(plans, coupons, jurisdictions)

    @classmethod
    def load(cls, path: str | Path = DEFAULT_CATALOG) -> "Catalog":
        with open(path, encoding="utf-8") as handle:
            return cls.from_dict(json.load(handle))

    def plan(self, code: str) -> Plan:
        try:
            return self.plans[code]
        except KeyError:
            raise KeyError(f"unknown plan: {code}") from None

    def coupon(self, code: str | None) -> Coupon | None:
        if code is None:
            return None
        try:
            return self.coupons[code]
        except KeyError:
            raise KeyError(f"unknown coupon: {code}") from None

    def jurisdiction(self, code: str) -> Jurisdiction:
        try:
            return self.jurisdictions[code]
        except KeyError:
            raise KeyError(f"unknown jurisdiction: {code}") from None
