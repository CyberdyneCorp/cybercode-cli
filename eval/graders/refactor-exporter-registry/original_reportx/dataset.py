"""The tabular data being exported, and loading it from JSON."""

import json
from dataclasses import dataclass

# A cell is one of these Python types; bool is checked before int where it matters.
CELL_TYPES = (str, int, float, bool, type(None))


class DatasetError(ValueError):
    """The input does not describe a valid dataset."""


@dataclass(frozen=True)
class Dataset:
    title: str
    columns: tuple  # unique, non-empty column names
    rows: tuple  # tuples of cells, each as long as `columns`

    def records(self) -> list[dict]:
        """Rows as dicts keyed by column name, in column order."""
        return [dict(zip(self.columns, row)) for row in self.rows]


def dataset_from_dict(data) -> Dataset:
    """Validate a decoded JSON object and build a Dataset."""
    if not isinstance(data, dict):
        raise DatasetError("dataset must be a JSON object")
    title = data.get("title", "")
    if not isinstance(title, str):
        raise DatasetError("title must be a string")
    columns = data.get("columns")
    if not isinstance(columns, list) or not columns:
        raise DatasetError("columns must be a non-empty list")
    for index, column in enumerate(columns):
        if not isinstance(column, str) or not column:
            raise DatasetError(f"column {index} must be a non-empty string")
    if len(set(columns)) != len(columns):
        raise DatasetError("column names must be unique")
    rows = data.get("rows", [])
    if not isinstance(rows, list):
        raise DatasetError("rows must be a list")
    checked = []
    for number, row in enumerate(rows, start=1):
        if not isinstance(row, list) or len(row) != len(columns):
            raise DatasetError(f"row {number} must be a list of {len(columns)} values")
        for value in row:
            if not isinstance(value, CELL_TYPES) or isinstance(value, (list, dict)):
                raise DatasetError(f"row {number} has an unsupported value: {value!r}")
        checked.append(tuple(row))
    return Dataset(title, tuple(columns), tuple(checked))


def load_dataset(path) -> Dataset:
    """Read a dataset from a UTF-8 JSON file."""
    with open(path, encoding="utf-8") as handle:
        try:
            data = json.load(handle)
        except json.JSONDecodeError as error:
            raise DatasetError(f"invalid JSON: {error.msg} (line {error.lineno})") from None
    return dataset_from_dict(data)
