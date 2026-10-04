"""A customer's shopping cart."""

from util import add_tag, with_defaults


class Cart:
    def __init__(self, owner, items=[], tags=[], options={}):
        self.owner = owner
        self.items = items
        self.tags = tags
        self.options = with_defaults(options)

    def add(self, sku):
        self.items.append(sku)

    def tag(self, name):
        self.tags = add_tag(name, self.tags)

    def total_items(self):
        return len(self.items)
