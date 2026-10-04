"""How users may name the output formats."""

from .exporters import get_exporter


def normalize_format(name: str) -> str:
    """Map a user-supplied format name (any case, surrounding spaces, aliases) to its canonical name.

    Raises ValueError for an unknown format.
    """
    return get_exporter(name).name
