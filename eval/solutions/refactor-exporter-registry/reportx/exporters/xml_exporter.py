from ..escape import xml_attribute, xml_text
from ..values import cell_text
from .base import Exporter, register


@register
class XmlExporter(Exporter):
    """A <report> element with one <row> of <field> elements per row."""

    name = "xml"
    extension = ".xml"
    content_type = "application/xml"
    header_row = False

    def render(self, dataset, header: bool) -> str:
        title = xml_attribute(dataset.title)
        lines = ['<?xml version="1.0" encoding="UTF-8"?>']
        if not dataset.rows:
            lines.append(f'<report title="{title}"/>')
        else:
            lines.append(f'<report title="{title}">')
            for row in dataset.rows:
                lines.append("  <row>")
                lines += [self._field(name, value) for name, value in zip(dataset.columns, row)]
                lines.append("  </row>")
            lines.append("</report>")
        return "".join(line + "\n" for line in lines)

    @staticmethod
    def _field(name: str, value) -> str:
        if value is None:
            return f'    <field name="{xml_attribute(name)}" null="true"/>'
        return f'    <field name="{xml_attribute(name)}">{xml_text(cell_text(value))}</field>'
