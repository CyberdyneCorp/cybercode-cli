"""Small helpers shared by the cart model."""

DEFAULT_OPTIONS = {"retries": 3, "timeout": 10}


def add_tag(tag, tags=None):
    """A new list: `tags` with `tag` appended unless already present."""
    result = list(tags or [])
    if tag not in result:
        result.append(tag)
    return result


def with_defaults(options=None):
    """A new dict: DEFAULT_OPTIONS overridden by `options`."""
    return {**DEFAULT_OPTIONS, **(options or {})}
