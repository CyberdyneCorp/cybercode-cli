"""Plain-text invoice rendering (the format is specified in README.md)."""
from __future__ import annotations

from .invoice import Invoice
from .money import format_amount

DESCRIPTION_WIDTH = 30
AMOUNT_WIDTH = 12
LABEL_WIDTH = 22 + 2 + DESCRIPTION_WIDTH  # "YYYY-MM-DD..YYYY-MM-DD" + gap + description


def _row(label: str, amount: str) -> str:
    return f"{label:<{LABEL_WIDTH}} {amount:>{AMOUNT_WIDTH}}"


def render_invoice(invoice: Invoice) -> str:
    cur = invoice.currency
    out = [f"INVOICE {invoice.customer} ({cur})"]
    for line in invoice.lines:
        period = f"{line.start.isoformat()}..{line.end.isoformat()}"
        description = line.description[:DESCRIPTION_WIDTH]
        out.append(_row(f"{period}  {description}", format_amount(line.amount, cur)))
    out.append(_row("Subtotal", format_amount(invoice.subtotal, cur)))
    if invoice.discount:
        out.append(_row(f"Discount {invoice.coupon.code}", format_amount(-invoice.discount, cur)))
    label = f"Tax {invoice.jurisdiction.label()}" if invoice.jurisdiction else "Tax"
    out.append(_row(label, format_amount(invoice.tax, cur)))
    out.append(_row("Total", format_amount(invoice.total, cur)))
    return "\n".join(out) + "\n"
