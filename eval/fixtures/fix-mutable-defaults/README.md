# cart

Shopping-cart model (Python 3.10+, stdlib only).

Contract:

- `util.add_tag(tag, tags=None)` returns a **new** list: the given tags (or none) followed by
  `tag`, unless `tag` is already present, in which case the result has the same tags as before.
  The list passed in is never modified.
- `util.with_defaults(options=None)` returns a new dict: `DEFAULT_OPTIONS` (`retries` 3,
  `timeout` 10) overridden by `options`. Neither `DEFAULT_OPTIONS` nor `options` is modified.
- `cart.Cart(owner, items=None, tags=None, options=None)` starts with its own copies of the
  given items, tags and options (`options` merged with `with_defaults`). `add(sku)` appends to the
  cart's items, `tag(name)` adds a tag with `add_tag`. Carts never share state with each other or
  with the lists and dicts the caller passed in.

Support reports that items added to one customer's cart show up in other customers' carts, and
that tags and options leak between calls. Run the tests with `python3 -m unittest`.
