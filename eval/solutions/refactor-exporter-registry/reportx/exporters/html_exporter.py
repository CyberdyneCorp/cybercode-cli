from ..escape import html_text
from ..values import is_number
from .base import Exporter, register


@register
class HtmlExporter(Exporter):
    """An HTML <table> with the title as caption; shown inline by browsers."""

    name = "html"
    aliases = ("htm",)
    extension = ".html"
    content_type = "text/html; charset=utf-8"
    disposition = "inline"

    def render(self, dataset, header: bool) -> str:
        lines = ["<table>"]
        if dataset.title:
            lines.append(f"  <caption>{html_text(dataset.title)}</caption>")
        if header:
            cells = "".join(f"<th>{html_text(column)}</th>" for column in dataset.columns)
            lines += ["  <thead>", f"    <tr>{cells}</tr>", "  </thead>"]
        if dataset.rows:
            lines.append("  <tbody>")
            lines += ["    <tr>" + "".join(map(self._cell, row)) + "</tr>" for row in dataset.rows]
            lines.append("  </tbody>")
        else:
            lines.append("  <tbody></tbody>")
        lines.append("</table>")
        return "".join(line + "\n" for line in lines)

    @staticmethod
    def _cell(value) -> str:
        if value is None:
            return '<td class="null"></td>'
        if is_number(value):
            return f'<td class="num">{html_text(value)}</td>'
        return f"<td>{html_text(value)}</td>"
