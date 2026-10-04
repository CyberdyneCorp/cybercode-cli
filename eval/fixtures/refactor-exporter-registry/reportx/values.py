"""Turning cells into text."""


def cell_text(value) -> str:
    """The plain-text form of a cell: "" for null, true/false, repr() for floats."""
    if value is None:
        return ""
    if value is True:
        return "true"
    if value is False:
        return "false"
    if isinstance(value, float):
        return repr(value)
    return str(value)


def is_number(value) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def numeric_columns(dataset) -> list[bool]:
    """For each column: True when it has at least one non-null value and all of them are numbers."""
    flags = []
    for index in range(len(dataset.columns)):
        values = [row[index] for row in dataset.rows if row[index] is not None]
        flags.append(bool(values) and all(is_number(v) for v in values))
    return flags
