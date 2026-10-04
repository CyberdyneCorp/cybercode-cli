import json

from .base import Exporter, register


@register
class NdjsonExporter(Exporter):
    """One compact JSON object per row, newline-separated, without a trailing newline."""

    name = "ndjson"
    aliases = ("jsonl",)
    extension = ".ndjson"
    content_type = "application/x-ndjson"
    header_row = False

    def render(self, dataset, header: bool) -> str:
        # ASCII-only so the lines survive log pipelines that are not UTF-8 clean.
        return "\n".join(json.dumps(record, ensure_ascii=True, separators=(",", ":"))
                         for record in dataset.records())
