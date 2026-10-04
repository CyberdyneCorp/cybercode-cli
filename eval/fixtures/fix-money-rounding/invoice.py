"""Invoice arithmetic. See README.md for the money rules."""


def line_total(unit_price, quantity):
    return f"{round(float(unit_price) * quantity, 2):.2f}"


def invoice_total(lines, tax_rate):
    subtotal = sum(float(price) * quantity for price, quantity in lines)
    tax = round(subtotal * float(tax_rate), 2)
    return {"subtotal": f"{subtotal:.2f}", "tax": f"{tax:.2f}", "total": f"{subtotal + tax:.2f}"}


def split_amount(total, n):
    share = round(float(total) / n, 2)
    return [f"{share:.2f}"] * n
