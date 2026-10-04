from ..escape import markdown_cell
from ..values import numeric_columns
from .base import Exporter, register


@register
class MarkdownExporter(Exporter):
    """A GitHub-style table under a title heading, with a row-count footer."""

    name = "markdown"
    aliases = ("md",)
    extension = ".md"
    content_type = "text/markdown; charset=utf-8"

    def render(self, dataset, header: bool) -> str:
        if not header:
            raise ValueError("markdown output requires a header row")
        lines = []
        if dataset.title:
            lines += ["# " + " ".join(dataset.title.split()), ""]
        lines.append(self._row(dataset.columns))
        lines.append("| " + " | ".join("---:" if flag else "---" for flag in numeric_columns(dataset)) + " |")
        lines += [self._row(row) for row in dataset.rows]
        count = len(dataset.rows)
        lines += ["", f"_{count} {'row' if count == 1 else 'rows'}_"]
        return "".join(line + "\n" for line in lines)

    @staticmethod
    def _row(values) -> str:
        return "| " + " | ".join(markdown_cell(value) for value in values) + " |"
