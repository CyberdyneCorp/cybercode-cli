"""HTTP response metadata for serving an exported report."""

from .exporters import get_exporter
from .files import output_filename


def content_type(fmt: str) -> str:
    """The Content-Type header value for a format."""
    return get_exporter(fmt).content_type


def response_headers(dataset, fmt: str) -> dict:
    """Headers for serving `dataset` rendered as `fmt`; the exporter decides inline vs. download."""
    exporter = get_exporter(fmt)
    return {
        "Content-Type": exporter.content_type,
        "Content-Disposition": f'{exporter.disposition}; filename="{output_filename(dataset.title, exporter.name)}"',
        "X-Row-Count": str(len(dataset.rows)),
    }
