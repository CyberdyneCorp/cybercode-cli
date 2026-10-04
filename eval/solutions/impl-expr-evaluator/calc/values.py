"""Value helpers shared by the evaluator and the builtins."""

from decimal import Decimal

TYPE_NAMES = ((bool, "bool"), (int, "int"), (Decimal, "decimal"), (str, "str"), (type(None), "null"))


def type_name(value) -> str:
    # bool is checked before int because bool is a subclass of int in Python.
    for python_type, name in TYPE_NAMES:
        if isinstance(value, python_type):
            return name
    raise TypeError(f"not a calc value: {value!r}")


def is_int(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def is_number(value) -> bool:
    return is_int(value) or isinstance(value, Decimal)


def truthy(value) -> bool:
    # Python's own truthiness matches the spec for every calc value (Decimal zeros are falsy).
    return bool(value)
