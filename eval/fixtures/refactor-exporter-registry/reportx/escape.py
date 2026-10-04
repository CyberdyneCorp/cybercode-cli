"""Escaping helpers for the individual output formats."""

import html
import re

from .values import cell_text

_XML_INVALID = re.compile("[\x00-\x08\x0b\x0c\x0e-\x1f\ufffe\uffff]")


def csv_field(value) -> str:
    """RFC 4180 style: quote when needed, double embedded quotes.

    An empty string is written as "" so that it differs from a null, which is written as nothing.
    """
    if value is None:
        return ""
    text = "" if value == "" else cell_text(value)
    if text == "" or any(c in text for c in ',"\r\n') or text != text.strip(" "):
        return '"' + text.replace('"', '""') + '"'
    return text


def tsv_field(value) -> str:
    """Backslash escapes for tab, newline, carriage return and backslash; null is \\N."""
    if value is None:
        return "\\N"
    text = cell_text(value)
    return (text.replace("\\", "\\\\").replace("\t", "\\t")
            .replace("\n", "\\n").replace("\r", "\\r"))


def markdown_cell(value) -> str:
    text = cell_text(value)
    text = text.replace("|", "\\|")
    text = text.replace("\r\n", "<br>").replace("\n", "<br>").replace("\r", "<br>")
    return text


def html_text(value) -> str:
    text = html.escape(cell_text(value), quote=True)
    return text.replace("\r\n", "<br>").replace("\n", "<br>")


def xml_text(text: str) -> str:
    text = _XML_INVALID.sub("", text)
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def xml_attribute(text: str) -> str:
    return xml_text(text).replace('"', "&quot;").replace("\n", "&#10;").replace("\r", "&#13;").replace("\t", "&#9;")
