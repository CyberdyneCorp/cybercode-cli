"""Rendering a dataset as text in one of the supported formats."""

from .exporters import get_exporter


def render(dataset, fmt: str, header: bool = True) -> str:
    """The complete document for `dataset` in format `fmt` (see the format's exporter)."""
    return get_exporter(fmt).render(dataset, header)
