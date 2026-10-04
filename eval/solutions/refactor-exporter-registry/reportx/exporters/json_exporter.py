import json

from .base import Exporter, register


@register
class JsonExporter(Exporter):
    """One indented UTF-8 JSON document with title, columns and row objects."""

    name = "json"
    extension = ".json"
    content_type = "application/json"
    header_row = False

    def render(self, dataset, header: bool) -> str:
        document = {"title": dataset.title, "columns": list(dataset.columns), "rows": dataset.records()}
        return json.dumps(document, indent=2, ensure_ascii=False) + "\n"
