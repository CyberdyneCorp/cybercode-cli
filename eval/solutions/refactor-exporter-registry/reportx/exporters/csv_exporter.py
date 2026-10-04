from ..escape import csv_field
from .base import Exporter, register


@register
class CsvExporter(Exporter):
    """RFC 4180 style CSV with CRLF line endings."""

    name = "csv"
    extension = ".csv"
    content_type = "text/csv; charset=utf-8"

    def render(self, dataset, header: bool) -> str:
        rows = ([dataset.columns] if header else []) + list(dataset.rows)
        return "".join(",".join(csv_field(value) for value in row) + "\r\n" for row in rows)
