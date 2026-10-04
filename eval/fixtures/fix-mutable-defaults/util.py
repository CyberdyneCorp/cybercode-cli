"""Small helpers shared by the cart model."""

DEFAULT_OPTIONS = {"retries": 3, "timeout": 10}


def add_tag(tag, tags=[]):
    """Tags with `tag` appended unless already present."""
    if tag not in tags:
        tags.append(tag)
    return tags


def with_defaults(options={}, base=DEFAULT_OPTIONS):
    """DEFAULT_OPTIONS overridden by `options`."""
    base.update(options)
    return base
