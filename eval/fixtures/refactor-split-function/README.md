# orders

`orders.py` turns the nightly order export into a text report. Everything happens in one
function, `process_orders(text)`, which has become hard to change and impossible to test
piece by piece.

Rules it implements: each row is `order_id,customer,sku,quantity,unit_price[,coupon]`;
invalid rows are reported as errors and skipped; coupon `SAVE10` takes 10% off the line,
`BULK` takes 5% off lines with quantity >= 10; tax is 8% of (subtotal - discount) per
customer; money is rounded half-up to cents.

Run the tests with `python3 -m unittest`.
