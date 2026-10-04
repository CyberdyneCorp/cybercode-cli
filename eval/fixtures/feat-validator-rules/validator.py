"""Validate decoded JSON against a small JSON-Schema-like dialect. See README.md."""

from typing import Any

TYPE_NAMES = {
    dict: "object",
    list: "array",
    str: "string",
    bool: "boolean",
    int: "integer",
    float: "number",
    type(None): "null",
}


def type_name(value: Any) -> str:
    return TYPE_NAMES.get(type(value), type(value).__name__)


def is_type(value: Any, expected: str) -> bool:
    actual = type_name(value)
    return actual == expected or (expected == "number" and actual == "integer")


def is_number(value: Any) -> bool:
    return type_name(value) in ("integer", "number")


def validate(schema: dict, data: Any, path: str = "$") -> list[str]:
    """Return every violation of `schema` in `data` as `<path>: <message>`."""
    expected = schema.get("type")
    if expected is not None and not is_type(data, expected):
        return [f"{path}: expected {expected}, got {type_name(data)}"]
    errors = []
    if is_number(data):
        if "minimum" in schema and data < schema["minimum"]:
            errors.append(f"{path}: must be >= {schema['minimum']}")
        if "maximum" in schema and data > schema["maximum"]:
            errors.append(f"{path}: must be <= {schema['maximum']}")
    if isinstance(data, dict):
        for name in schema.get("required", []):
            if name not in data:
                errors.append(f"{path}: missing required property '{name}'")
        for name, subschema in schema.get("properties", {}).items():
            if name in data:
                errors.extend(validate(subschema, data[name], f"{path}.{name}"))
    return errors
