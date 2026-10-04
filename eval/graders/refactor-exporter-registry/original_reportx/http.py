"""HTTP response metadata for serving an exported report."""

from .files import output_filename
from .formats import normalize_format


def content_type(fmt: str) -> str:
    """The Content-Type header value for a format."""
    fmt = normalize_format(fmt)
    if fmt == "csv":
        return "text/csv; charset=utf-8"
    if fmt == "tsv":
        return "text/tab-separated-values; charset=utf-8"
    if fmt == "json":
        return "application/json"
    if fmt == "ndjson":
        return "application/x-ndjson"
    if fmt == "markdown":
        return "text/markdown; charset=utf-8"
    if fmt == "html":
        return "text/html; charset=utf-8"
    return "application/xml"


def response_headers(dataset, fmt: str) -> dict:
    """Headers for serving `dataset` rendered as `fmt`.

    HTML is shown in the browser (inline); every other format is offered as a download.
    """
    fmt = normalize_format(fmt)
    disposition = "inline" if fmt == "html" else "attachment"
    filename = output_filename(dataset.title, fmt)
    return {
        "Content-Type": content_type(fmt),
        "Content-Disposition": f'{disposition}; filename="{filename}"',
        "X-Row-Count": str(len(dataset.rows)),
    }
