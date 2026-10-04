"""Tree-walking evaluator (see SPEC.md, Evaluation)."""

import decimal
from decimal import Decimal

from . import functions
from .errors import CalcError
from .parser import Binary, Call, Compare, Conditional, Let, Literal, Logical, Name, Not, Unary, parse
from .values import is_int, is_number, truthy, type_name

CONTEXT = decimal.Context(prec=28, rounding=decimal.ROUND_HALF_EVEN)
_MISSING = object()


def evaluate(source: str, env: dict | None = None):
    """Evaluate one calc expression; see SPEC.md for the full contract."""
    tree = parse(source)
    with decimal.localcontext(CONTEXT):
        return _Evaluator(dict(env or {})).eval(tree)


class _Scope:
    def __init__(self, variables: dict, parent=None):
        self.variables = variables
        self.parent = parent

    def lookup(self, name: str):
        scope = self
        while scope is not None:
            if name in scope.variables:
                return scope.variables[name]
            scope = scope.parent
        return _MISSING


class _Evaluator:
    def __init__(self, env: dict):
        self.scope = _Scope(env)

    def eval(self, node):
        return getattr(self, f"eval_{type(node).__name__}")(node)

    def eval_Literal(self, node: Literal):
        return node.value

    def eval_Name(self, node: Name):
        value = self.scope.lookup(node.name)
        if value is _MISSING:
            raise CalcError(f"undefined variable '{node.name}'", node.column)
        return value

    def eval_Let(self, node: Let):
        value = self.eval(node.value)
        outer = self.scope
        self.scope = _Scope({node.name: value}, outer)
        try:
            return self.eval(node.body)
        finally:
            self.scope = outer

    def eval_Conditional(self, node: Conditional):
        branch = node.then if truthy(self.eval(node.condition)) else node.otherwise
        return self.eval(branch)

    def eval_Logical(self, node: Logical):
        left = self.eval(node.left)
        if truthy(left) == (node.op == "or"):
            return left
        return self.eval(node.right)

    def eval_Not(self, node: Not):
        return not truthy(self.eval(node.operand))

    def eval_Compare(self, node: Compare):
        left = self.eval(node.operands[0])
        for (op, column), operand in zip(node.ops, node.operands[1:]):
            right = self.eval(operand)
            if not compare(op, column, left, right):
                return False
            left = right
        return True

    def eval_Unary(self, node: Unary):
        value = self.eval(node.operand)
        if is_int(value):
            return -value if node.op == "-" else value
        if isinstance(value, Decimal):
            return value.copy_negate() if node.op == "-" else value
        raise CalcError(f"bad operand type for unary {node.op}: {type_name(value)}", node.column)

    def eval_Binary(self, node: Binary):
        left = self.eval(node.left)
        right = self.eval(node.right)
        return arithmetic(node.op, node.column, left, right)

    def eval_Call(self, node: Call):
        function = functions.check_call(node.name, node.column, len(node.args))
        args = [self.eval(arg) for arg in node.args]
        return functions.call(function, args, node.column)


def _type_error(op: str, column: int, left, right) -> CalcError:
    return CalcError(f"unsupported operand types for {op}: {type_name(left)} and {type_name(right)}", column)


def compare(op: str, column: int, left, right) -> bool:
    if op in ("==", "!="):
        return equal(left, right) == (op == "==")
    orderable = (is_number(left) and is_number(right)) or (isinstance(left, str) and isinstance(right, str))
    if not orderable:
        raise _type_error(op, column, left, right)
    return {"<": left < right, "<=": left <= right, ">": left > right, ">=": left >= right}[op]


def equal(left, right) -> bool:
    if is_number(left) and is_number(right):
        return left == right
    return type_name(left) == type_name(right) and left == right


def arithmetic(op: str, column: int, left, right):
    if op == "+" and isinstance(left, str) and isinstance(right, str):
        return left + right
    if op == "*" and isinstance(left, str) and is_int(right):
        return left * right
    if op == "*" and is_int(left) and isinstance(right, str):
        return left * right
    if op == "**":
        return _power(column, left, right)
    if not (is_number(left) and is_number(right)):
        raise _type_error(op, column, left, right)
    if op in ("/", "//", "%") and right == 0:
        raise CalcError("division by zero", column)
    if op == "/":
        return Decimal(left) / Decimal(right)
    if op in ("//", "%") and not (is_int(left) and is_int(right)):
        quotient, remainder = _floor_divmod(Decimal(left), Decimal(right))
        return quotient if op == "//" else remainder
    return {"+": lambda: left + right, "-": lambda: left - right, "*": lambda: left * right,
            "//": lambda: left // right, "%": lambda: left % right}[op]()


def _floor_divmod(a: Decimal, b: Decimal) -> tuple[Decimal, Decimal]:
    quotient, remainder = a // b, a % b
    if remainder and (remainder < 0) != (b < 0):
        quotient, remainder = quotient - 1, remainder + b
    return quotient, remainder


def _power(column: int, base, exponent):
    if not (is_number(base) and is_int(exponent)):
        raise _type_error("**", column, base, exponent)
    if exponent == 0:
        return 1 if is_int(base) else Decimal(1)
    if exponent < 0 and base == 0:
        raise CalcError("division by zero", column)
    if is_int(base) and exponent > 0:
        return base ** exponent
    return Decimal(base) ** exponent
