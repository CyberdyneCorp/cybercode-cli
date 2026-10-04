"""Nightly order report.

Input: one order line per row, `order_id,customer,sku,quantity,unit_price[,coupon]`.
Blank rows and rows starting with `#` are ignored. See README.md for the rules.
"""

from decimal import Decimal, InvalidOperation, ROUND_HALF_UP

CENT = Decimal("0.01")
TAX_RATE = Decimal("0.08")
COUPONS = {"SAVE10", "BULK"}


def process_orders(text: str) -> str:
    """Parse, validate, total and render the order report for `text`."""
    errors = []
    seen = set()
    customers = {}
    for number, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = [part.strip() for part in line.split(",")]
        if len(parts) not in (5, 6):
            errors.append(f"line {number}: expected 5 or 6 fields, got {len(parts)}")
            continue
        order_id, customer, sku, quantity_text, price_text = parts[:5]
        coupon = parts[5] if len(parts) == 6 else ""
        line_errors = []
        if not order_id:
            line_errors.append(f"line {number}: missing order id")
        if not customer:
            line_errors.append(f"line {number}: missing customer")
        try:
            quantity = int(quantity_text)
            if quantity <= 0:
                raise ValueError
        except ValueError:
            line_errors.append(f"line {number}: quantity must be a positive integer")
            quantity = 0
        try:
            price = Decimal(price_text)
            if not price.is_finite() or price < 0:
                raise InvalidOperation
        except InvalidOperation:
            line_errors.append(f"line {number}: invalid price {price_text!r}")
            price = Decimal(0)
        if coupon and coupon not in COUPONS:
            line_errors.append(f"line {number}: unknown coupon {coupon!r}")
        if not line_errors and (order_id, sku) in seen:
            line_errors.append(f"line {number}: duplicate line for order {order_id} sku {sku}")
        if line_errors:
            errors.extend(line_errors)
            continue
        seen.add((order_id, sku))
        amount = (price * quantity).quantize(CENT, ROUND_HALF_UP)
        if coupon == "SAVE10":
            discount = (amount * Decimal("0.10")).quantize(CENT, ROUND_HALF_UP)
        elif coupon == "BULK" and quantity >= 10:
            discount = (amount * Decimal("0.05")).quantize(CENT, ROUND_HALF_UP)
        else:
            discount = Decimal("0.00")
        entry = customers.setdefault(customer, {"lines": 0, "subtotal": Decimal("0.00"), "discount": Decimal("0.00")})
        entry["lines"] += 1
        entry["subtotal"] += amount
        entry["discount"] += discount
    out = ["ORDER REPORT", "============"]
    grand = {"lines": 0, "subtotal": Decimal("0.00"), "discount": Decimal("0.00"), "tax": Decimal("0.00"), "total": Decimal("0.00")}
    for customer in sorted(customers):
        entry = customers[customer]
        taxable = entry["subtotal"] - entry["discount"]
        tax = (taxable * TAX_RATE).quantize(CENT, ROUND_HALF_UP)
        total = taxable + tax
        out.append(
            f"customer {customer}: {entry['lines']} line{'s' if entry['lines'] != 1 else ''}, "
            f"subtotal {entry['subtotal']}, discount {entry['discount']}, tax {tax}, total {total}"
        )
        grand["lines"] += entry["lines"]
        grand["subtotal"] += entry["subtotal"]
        grand["discount"] += entry["discount"]
        grand["tax"] += tax
        grand["total"] += total
    if not customers:
        out.append("no valid orders")
    out.append("------------")
    out.append(f"lines: {grand['lines']}  customers: {len(customers)}")
    for key in ("subtotal", "discount", "tax", "total"):
        out.append(f"{key} {grand[key]}")
    if errors:
        out.append(f"errors ({len(errors)}):")
        out.extend(f"  {error}" for error in errors)
    else:
        out.append("errors: none")
    return "\n".join(out) + "\n"
