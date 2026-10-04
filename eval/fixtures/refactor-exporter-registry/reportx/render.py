"""Rendering a dataset as text in one of the supported formats."""

import json

from .escape import csv_field, html_text, markdown_cell, tsv_field, xml_attribute, xml_text
from .formats import normalize_format
from .values import cell_text, is_number, numeric_columns


def render(dataset, fmt: str, header: bool = True) -> str:
    """The complete document for `dataset` in format `fmt`.

    `header=False` leaves out the column-name row where a format has one (csv, tsv, html);
    json, ndjson and xml always name their fields, and markdown cannot do without a header.
    """
    fmt = normalize_format(fmt)
    if fmt == "json":
        return _json_document(dataset)
    if fmt == "ndjson":
        return "\n".join(_ndjson_line(record) for record in dataset.records())
    if fmt == "markdown" and not header:
        raise ValueError("markdown output requires a header row")

    numeric = numeric_columns(dataset)
    lines = _header_lines(dataset, fmt, header, numeric)
    for row in dataset.rows:
        lines.append(_row_line(dataset.columns, row, fmt))
    lines += _footer_lines(dataset, fmt)
    newline = "\r\n" if fmt == "csv" else "\n"
    return "".join(line + newline for line in lines)


def _json_document(dataset) -> str:
    document = {"title": dataset.title, "columns": list(dataset.columns), "rows": dataset.records()}
    return json.dumps(document, indent=2, ensure_ascii=False) + "\n"


def _ndjson_line(record: dict) -> str:
    # ASCII-only so the lines survive log pipelines that are not UTF-8 clean.
    return json.dumps(record, ensure_ascii=True, separators=(",", ":"))


def _header_lines(dataset, fmt: str, header: bool, numeric: list) -> list:
    if fmt == "csv":
        return [",".join(csv_field(c) for c in dataset.columns)] if header else []
    elif fmt == "tsv":
        return ["\t".join(tsv_field(c) for c in dataset.columns)] if header else []
    elif fmt == "markdown":
        lines = []
        if dataset.title:
            lines += ["# " + " ".join(dataset.title.split()), ""]
        lines.append("| " + " | ".join(markdown_cell(c) for c in dataset.columns) + " |")
        lines.append("| " + " | ".join("---:" if flag else "---" for flag in numeric) + " |")
        return lines
    elif fmt == "html":
        lines = ["<table>"]
        if dataset.title:
            lines.append(f"  <caption>{html_text(dataset.title)}</caption>")
        if header:
            cells = "".join(f"<th>{html_text(c)}</th>" for c in dataset.columns)
            lines += ["  <thead>", f"    <tr>{cells}</tr>", "  </thead>"]
        lines.append("  <tbody>" if dataset.rows else "  <tbody></tbody>")
        return lines
    else:
        lines = ['<?xml version="1.0" encoding="UTF-8"?>']
        title = xml_attribute(dataset.title)
        lines.append(f'<report title="{title}">' if dataset.rows else f'<report title="{title}"/>')
        return lines


def _row_line(columns: tuple, row: tuple, fmt: str) -> str:
    if fmt == "csv":
        return ",".join(csv_field(value) for value in row)
    elif fmt == "tsv":
        return "\t".join(tsv_field(value) for value in row)
    elif fmt == "markdown":
        return "| " + " | ".join(markdown_cell(value) for value in row) + " |"
    elif fmt == "html":
        return "    <tr>" + "".join(_html_cell(value) for value in row) + "</tr>"
    else:
        return "\n".join(["  <row>", *(_xml_field(name, value) for name, value in zip(columns, row)), "  </row>"])


def _footer_lines(dataset, fmt: str) -> list:
    if fmt == "markdown":
        count = len(dataset.rows)
        return ["", f"_{count} {'row' if count == 1 else 'rows'}_"]
    elif fmt == "html":
        return (["  </tbody>"] if dataset.rows else []) + ["</table>"]
    elif fmt == "xml":
        return ["</report>"] if dataset.rows else []
    return []


def _html_cell(value) -> str:
    if value is None:
        return '<td class="null"></td>'
    if is_number(value):
        return f'<td class="num">{html_text(value)}</td>'
    return f"<td>{html_text(value)}</td>"


def _xml_field(name: str, value) -> str:
    if value is None:
        return f'    <field name="{xml_attribute(name)}" null="true"/>'
    return f'    <field name="{xml_attribute(name)}">{xml_text(cell_text(value))}</field>'
