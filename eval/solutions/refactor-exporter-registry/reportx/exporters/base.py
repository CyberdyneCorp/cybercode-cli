"""The exporter plugin API: one Exporter subclass per output format, registered by name.

Adding a format means adding one module to this package that defines an `Exporter` subclass
decorated with `@register`; the package imports every module in it on first use.
"""

_EXPORTERS: dict[str, "Exporter"] = {}  # canonical name -> exporter
_LOOKUP: dict[str, "Exporter"] = {}  # canonical name or alias -> exporter


class Exporter:
    """Base class for an output format. Subclasses set the attributes and implement render()."""

    name: str  # canonical, lowercase format name, e.g. "csv"
    extension: str  # file extension including the dot, e.g. ".csv"
    content_type: str  # Content-Type header value
    aliases: tuple = ()  # other lowercase names accepted for the format
    disposition: str = "attachment"  # Content-Disposition type
    header_row: bool = True  # False when the output does not depend on `header`

    def render(self, dataset, header: bool) -> str:
        """The complete document for `dataset`; `header=False` drops the column-name row."""
        raise NotImplementedError


def register(cls):
    """Class decorator: register an instance of `cls` under its name and aliases."""
    exporter = cls()
    keys = [exporter.name, *exporter.aliases]
    taken = [key for key in keys if key in _LOOKUP]
    if taken or len(set(keys)) != len(keys):
        raise ValueError(f"format name already registered: {', '.join(taken or keys)}")
    _EXPORTERS[exporter.name] = exporter
    for key in keys:
        _LOOKUP[key] = exporter
    return cls


def get_exporter(name: str) -> Exporter:
    """The exporter for a format name or alias (case-insensitive, surrounding spaces ignored)."""
    exporter = _LOOKUP.get(name.strip().lower())
    if exporter is None:
        choices = ", ".join(available_formats())
        raise ValueError(f"unknown format {name!r} (choose from {choices})")
    return exporter


def available_formats() -> list[str]:
    """Canonical names of all registered formats, sorted."""
    return sorted(_EXPORTERS)
