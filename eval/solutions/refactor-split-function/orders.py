"""Nightly order report.

Input: one order line per row, `order_id,customer,sku,quantity,unit_price[,coupon]`.
Blank rows and rows starting with `#` are ignored. See README.md for the rules.
"""

from dataclasses import dataclass, field
from decimal import Decimal, InvalidOperation, ROUND_HALF_UP

CENT = Decimal("0.01")
ZERO = Decimal("0.00")
TAX_RATE = Decimal("0.08")
COUPONS = {"SAVE10", "BULK"}


@dataclass
class Order:
    """One non-blank, non-comment input row."""

    number: int
    fields: list[str]
    order_id: str = ""
    customer: str = ""
    sku: str = ""
    quantity: int = 0
    price: Decimal = ZERO
    coupon: str = ""


@dataclass
class CustomerTotals:
    lines: int = 0
    subtotal: Decimal = ZERO
    discount: Decimal = ZERO

    @property
    def tax(self) -> Decimal:
        return ((self.subtotal - self.discount) * TAX_RATE).quantize(CENT, ROUND_HALF_UP)

    @property
    def total(self) -> Decimal:
        return self.subtotal - self.discount + self.tax


@dataclass
class Totals:
    customers: dict[str, CustomerTotals] = field(default_factory=dict)


def parse_orders(text: str) -> list[Order]:
    """Split `text` into rows, keeping 1-based line numbers."""
    orders = []
    for number, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if line and not line.startswith("#"):
            orders.append(Order(number, [part.strip() for part in line.split(",")]))
    return orders


def parse_quantity(text: str) -> int | None:
    try:
        quantity = int(text)
    except ValueError:
        return None
    return quantity if quantity > 0 else None


def parse_price(text: str) -> Decimal | None:
    try:
        price = Decimal(text)
    except InvalidOperation:
        return None
    return price if price.is_finite() and price >= 0 else None


def validate_order(order: Order) -> list[str]:
    """Check one row and fill in its typed fields; return its error messages."""
    prefix = f"line {order.number}:"
    if len(order.fields) not in (5, 6):
        return [f"{prefix} expected 5 or 6 fields, got {len(order.fields)}"]
    order.order_id, order.customer, order.sku, quantity_text, price_text = order.fields[:5]
    order.coupon = order.fields[5] if len(order.fields) == 6 else ""
    quantity, price = parse_quantity(quantity_text), parse_price(price_text)
    errors = []
    if not order.order_id:
        errors.append(f"{prefix} missing order id")
    if not order.customer:
        errors.append(f"{prefix} missing customer")
    if quantity is None:
        errors.append(f"{prefix} quantity must be a positive integer")
    if price is None:
        errors.append(f"{prefix} invalid price {price_text!r}")
    if order.coupon and order.coupon not in COUPONS:
        errors.append(f"{prefix} unknown coupon {order.coupon!r}")
    order.quantity, order.price = quantity or 0, price if price is not None else ZERO
    return errors


def line_discount(amount: Decimal, order: Order) -> Decimal:
    if order.coupon == "SAVE10":
        return (amount * Decimal("0.10")).quantize(CENT, ROUND_HALF_UP)
    if order.coupon == "BULK" and order.quantity >= 10:
        return (amount * Decimal("0.05")).quantize(CENT, ROUND_HALF_UP)
    return ZERO


def compute_totals(orders: list[Order]) -> Totals:
    """Accumulate per-customer totals over valid rows."""
    totals = Totals()
    for order in orders:
        amount = (order.price * order.quantity).quantize(CENT, ROUND_HALF_UP)
        entry = totals.customers.setdefault(order.customer, CustomerTotals())
        entry.lines += 1
        entry.subtotal += amount
        entry.discount += line_discount(amount, order)
    return totals


def accept_valid(orders: list[Order]) -> tuple[list[Order], list[str]]:
    """Validate every row, dropping invalid rows and repeated (order_id, sku) pairs."""
    valid, errors, seen = [], [], set()
    for order in orders:
        problems = validate_order(order)
        if not problems and (order.order_id, order.sku) in seen:
            problems = [f"line {order.number}: duplicate line for order {order.order_id} sku {order.sku}"]
        if problems:
            errors.extend(problems)
            continue
        seen.add((order.order_id, order.sku))
        valid.append(order)
    return valid, errors


def render_customer(name: str, entry: CustomerTotals) -> str:
    plural = "s" if entry.lines != 1 else ""
    return (f"customer {name}: {entry.lines} line{plural}, subtotal {entry.subtotal}, "
            f"discount {entry.discount}, tax {entry.tax}, total {entry.total}")


def render_report(orders: list[Order], totals: Totals, errors: list[str]) -> str:
    """Render the report for the valid `orders`, their `totals` and the row `errors`."""
    customers = totals.customers
    out = ["ORDER REPORT", "============"]
    out += [render_customer(name, customers[name]) for name in sorted(customers)]
    if not customers:
        out.append("no valid orders")
    out += ["------------", f"lines: {len(orders)}  customers: {len(customers)}"]
    for key in ("subtotal", "discount", "tax", "total"):
        out.append(f"{key} {sum((getattr(e, key) for e in customers.values()), ZERO)}")
    if errors:
        out.append(f"errors ({len(errors)}):")
        out += [f"  {error}" for error in errors]
    else:
        out.append("errors: none")
    return "\n".join(out) + "\n"


def process_orders(text: str) -> str:
    """Parse, validate, total and render the order report for `text`."""
    valid, errors = accept_valid(parse_orders(text))
    return render_report(valid, compute_totals(valid), errors)
