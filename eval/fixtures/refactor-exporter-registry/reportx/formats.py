"""The supported output formats and how users may name them."""

FORMATS = ("csv", "tsv", "json", "ndjson", "markdown", "html", "xml")


def normalize_format(name: str) -> str:
    """Map a user-supplied format name (any case, surrounding spaces, aliases) to its canonical name.

    Raises ValueError for an unknown format.
    """
    key = name.strip().lower()
    if key == "md":
        key = "markdown"
    elif key == "jsonl":
        key = "ndjson"
    elif key == "htm":
        key = "html"
    if key not in FORMATS:
        choices = ", ".join(sorted(FORMATS))
        raise ValueError(f"unknown format {name!r} (choose from {choices})")
    return key
