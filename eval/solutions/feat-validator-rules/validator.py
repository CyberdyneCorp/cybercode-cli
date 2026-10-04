"""Validate decoded JSON against a small JSON-Schema-like dialect. See README.md."""

import json
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


def same_json_type(a: Any, b: Any) -> bool:
    return type_name(a) == type_name(b) or (is_number(a) and is_number(b))


def in_enum(value: Any, allowed: list) -> bool:
    """Equal to a member and of the same JSON type, so `1` never matches `true`."""
    return any(value == option and same_json_type(value, option) for option in allowed)


def validate(schema: dict, data: Any, path: str = "$") -> list[str]:
    """Return every violation of `schema` in `data` as `<path>: <message>`."""
    expected = schema.get("type")
    if expected is not None and not is_type(data, expected):
        return [f"{path}: expected {expected}, got {type_name(data)}"]
    errors = []
    if "enum" in schema and not in_enum(data, schema["enum"]):
        errors.append(f"{path}: must be one of {json.dumps(schema['enum'])}")
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
    if isinstance(data, list) and "items" in schema:
        for index, item in enumerate(data):
            errors.extend(validate(schema["items"], item, f"{path}[{index}]"))
    return errors
