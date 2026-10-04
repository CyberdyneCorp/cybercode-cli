"""Builtin functions (see SPEC.md, Builtin functions)."""

import re
from decimal import ROUND_DOWN, ROUND_HALF_EVEN, Decimal

from .errors import CalcError
from .values import is_int, is_number, type_name

_INT_TEXT = re.compile(r"[+-]?[0-9]+")


class _Failure(Exception):
    """Raised by a builtin; the evaluator turns it into a CalcError at the call's column."""


def _len(s):
    if not isinstance(s, str):
        raise _Failure(f"len() argument must be str, not {type_name(s)}")
    return len(s)


def _abs(x):
    if not is_number(x):
        raise _Failure(f"abs() argument must be a number, not {type_name(x)}")
    return abs(x) if is_int(x) else x.copy_abs()


def _extreme(name, better):
    def pick(*args):
        if not (all(map(is_number, args)) or all(isinstance(a, str) for a in args)):
            raise _Failure(f"{name}() arguments must be all numbers or all strings")
        best = args[0]
        for candidate in args[1:]:
            if better(candidate, best):
                best = candidate
        return best
    return pick


def _round(x, *ndigits):
    if not is_number(x):
        raise _Failure(f"round() argument must be a number, not {type_name(x)}")
    if ndigits and not is_int(ndigits[0]):
        raise _Failure(f"round() ndigits must be int, not {type_name(ndigits[0])}")
    if not ndigits:
        return x if is_int(x) else int(x.to_integral_value(rounding=ROUND_HALF_EVEN))
    n = ndigits[0]
    if is_int(x):
        if n >= 0:
            return x
        step = 10 ** -n
        quotient, remainder = divmod(x, step)
        if 2 * remainder > step or (2 * remainder == step and quotient % 2):
            quotient += 1
        return quotient * step
    return x.quantize(Decimal(1).scaleb(-n), rounding=ROUND_HALF_EVEN)


def _str(x):
    if isinstance(x, bool):
        return "true" if x else "false"
    if x is None:
        return "null"
    return str(x)


def _int(x):
    if is_int(x):
        return x
    if isinstance(x, Decimal):
        return int(x.to_integral_value(rounding=ROUND_DOWN))
    if isinstance(x, str):
        if not _INT_TEXT.fullmatch(x):
            raise _Failure(f"invalid literal for int(): '{x}'")
        return int(x)
    raise _Failure(f"int() argument must be a number or str, not {type_name(x)}")


def _exactly_one(k):
    return None if k == 1 else f"expects 1 argument, got {k}"


# name -> (function, arity check returning an error suffix or None)
BUILTINS = {
    "len": (_len, _exactly_one),
    "abs": (_abs, _exactly_one),
    "str": (_str, _exactly_one),
    "int": (_int, _exactly_one),
    "round": (_round, lambda k: None if k in (1, 2) else f"expects 1 or 2 arguments, got {k}"),
    "min": (_extreme("min", lambda a, b: a < b), lambda k: None if k else "expects at least 1 argument, got 0"),
    "max": (_extreme("max", lambda a, b: a > b), lambda k: None if k else "expects at least 1 argument, got 0"),
}


def check_call(name: str, column: int, argc: int):
    """Return the builtin, raising CalcError for an unknown name or a wrong argument count."""
    if name not in BUILTINS:
        raise CalcError(f"unknown function '{name}'", column)
    function, arity = BUILTINS[name]
    problem = arity(argc)
    if problem:
        raise CalcError(f"{name}() {problem}", column)
    return function


def call(function, args: list, column: int):
    try:
        return function(*args)
    except _Failure as failure:
        raise CalcError(str(failure), column) from None
