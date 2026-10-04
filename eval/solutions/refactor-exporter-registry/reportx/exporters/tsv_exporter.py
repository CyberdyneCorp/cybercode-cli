from ..escape import tsv_field
from .base import Exporter, register


@register
class TsvExporter(Exporter):
    """Tab-separated values with backslash escapes and \\N for null."""

    name = "tsv"
    extension = ".tsv"
    content_type = "text/tab-separated-values; charset=utf-8"

    def render(self, dataset, header: bool) -> str:
        rows = ([dataset.columns] if header else []) + list(dataset.rows)
        return "".join("\t".join(tsv_field(value) for value in row) + "\n" for row in rows)
