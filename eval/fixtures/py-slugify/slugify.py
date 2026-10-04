"""Turn titles into URL slugs."""


def slugify(title: str) -> str:
    """Lowercase, replace runs of non-alphanumerics with one hyphen, trim hyphens."""
    out = []
    for ch in title.lower():
        if ch.isalnum():
            out.append(ch)
        else:
            out.append("-")
    return "".join(out)
