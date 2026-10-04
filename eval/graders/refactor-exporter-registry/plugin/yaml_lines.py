"""A format added by the grader: it must be picked up just by dropping this module in."""

import json

from reportx.exporters.base import Exporter, register


@register
class YamlLinesExporter(Exporter):
    name = "yaml"
    aliases = ("yml",)
    extension = ".yaml"
    content_type = "application/yaml"
    disposition = "inline"
    header_row = False

    def render(self, dataset, header: bool) -> str:
        lines = [f"title: {json.dumps(dataset.title)}", "rows:"]
        lines += ["  - " + json.dumps(record) for record in dataset.records()]
        return "\n".join(lines) + "\n"
