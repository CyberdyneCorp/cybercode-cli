"""Machine-readable invoice export for the accounting system."""
from __future__ import annotations

import json

from .invoice import Invoice
from .money import format_amount


def invoice_to_dict(invoice: Invoice) -> dict:
    """A JSON-ready dict; every amount is a string with the currency's decimals."""
    cur = invoice.currency
    return {
        "customer": invoice.customer,
        "currency": cur,
        "lines": [
            {
                "kind": line.kind,
                "description": line.description,
                "start": line.start.isoformat(),
                "end": line.end.isoformat(),
                "amount": format_amount(line.amount, cur),
            }
            for line in invoice.lines
        ],
        "subtotal": format_amount(invoice.subtotal, cur),
        "coupon": invoice.coupon.code if invoice.coupon else None,
        "discount": format_amount(invoice.discount, cur),
        "jurisdiction": invoice.jurisdiction.code if invoice.jurisdiction else None,
        "tax": format_amount(invoice.tax, cur),
        "total": format_amount(invoice.total, cur),
    }


def invoices_to_jsonl(invoices) -> str:
    """One compact JSON object per line, keys in the order above."""
    return "".join(json.dumps(invoice_to_dict(inv), separators=(",", ":")) + "\n" for inv in invoices)
