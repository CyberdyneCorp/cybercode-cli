"""Invoices."""

from .users import fetch_user_data

PRICES = {"free": 0, "pro": 12}


def monthly_charge(user_id: int) -> int:
    user = fetch_user_data(user_id)
    return PRICES[user["plan"]] if user["active"] else 0


def invoice_header(user_id: int) -> str:
    user = fetch_user_data(user_id)
    return f"Invoice for {user['name']} ({user['plan']}): ${monthly_charge(user_id)}"
