"""Recursive-descent parser producing a small AST (see SPEC.md, Grammar)."""

from dataclasses import dataclass

from .errors import CalcError
from .lexer import Token, tokenize

COMPARISONS = ("==", "!=", "<", "<=", ">", ">=")


@dataclass(frozen=True)
class Literal:
    value: object


@dataclass(frozen=True)
class Name:
    name: str
    column: int


@dataclass(frozen=True)
class Let:
    name: str
    value: object
    body: object


@dataclass(frozen=True)
class Conditional:
    then: object
    condition: object
    otherwise: object


@dataclass(frozen=True)
class Logical:
    op: str  # "and" or "or"
    left: object
    right: object


@dataclass(frozen=True)
class Not:
    operand: object


@dataclass(frozen=True)
class Compare:
    operands: tuple
    ops: tuple  # tuple of (operator, column), one fewer than operands


@dataclass(frozen=True)
class Binary:
    op: str
    column: int
    left: object
    right: object


@dataclass(frozen=True)
class Unary:
    op: str
    column: int
    operand: object


@dataclass(frozen=True)
class Call:
    name: str
    column: int
    args: tuple


def parse(source: str):
    """Parse one complete expression; raise CalcError on lexical or syntax errors."""
    return _Parser(tokenize(source)).parse_all()


class _Parser:
    def __init__(self, tokens: list[Token]):
        self.tokens = tokens
        self.pos = 0

    # -- token helpers -------------------------------------------------------------------

    def peek(self) -> Token:
        return self.tokens[self.pos]

    def at(self, *texts: str) -> bool:
        token = self.peek()
        return token.kind in ("op", "keyword") and token.text in texts

    def advance(self) -> Token:
        token = self.peek()
        self.pos += 1
        return token

    def expect(self, text: str) -> Token:
        if not self.at(text):
            self.fail()
        return self.advance()

    def fail(self):
        token = self.peek()
        if token.kind == "eof":
            raise CalcError("unexpected end of input", token.column)
        raise CalcError(f"unexpected token '{token.text}'", token.column)

    # -- grammar -------------------------------------------------------------------------

    def parse_all(self):
        tree = self.expr()
        if self.peek().kind != "eof":
            self.fail()
        return tree

    def expr(self):
        if self.at("let"):
            return self.let_expr()
        return self.conditional()

    def let_expr(self):
        self.expect("let")
        if self.peek().kind != "ident":
            self.fail()
        name = self.advance().text
        self.expect("=")
        value = self.expr()
        self.expect("in")
        return Let(name, value, self.expr())

    def conditional(self):
        then = self.or_expr()
        if not self.at("if"):
            return then
        self.advance()
        condition = self.or_expr()
        self.expect("else")
        return Conditional(then, condition, self.expr())

    def or_expr(self):
        left = self.and_expr()
        while self.at("or"):
            self.advance()
            left = Logical("or", left, self.and_expr())
        return left

    def and_expr(self):
        left = self.not_expr()
        while self.at("and"):
            self.advance()
            left = Logical("and", left, self.not_expr())
        return left

    def not_expr(self):
        if self.at("not"):
            self.advance()
            return Not(self.not_expr())
        return self.comparison()

    def comparison(self):
        operands = [self.sum()]
        ops = []
        while self.at(*COMPARISONS):
            token = self.advance()
            ops.append((token.text, token.column))
            operands.append(self.sum())
        if not ops:
            return operands[0]
        return Compare(tuple(operands), tuple(ops))

    def sum(self):
        left = self.term()
        while self.at("+", "-"):
            token = self.advance()
            left = Binary(token.text, token.column, left, self.term())
        return left

    def term(self):
        left = self.unary()
        while self.at("*", "/", "//", "%"):
            token = self.advance()
            left = Binary(token.text, token.column, left, self.unary())
        return left

    def unary(self):
        if self.at("-", "+"):
            token = self.advance()
            return Unary(token.text, token.column, self.unary())
        return self.power()

    def power(self):
        base = self.primary()
        if not self.at("**"):
            return base
        token = self.advance()
        return Binary("**", token.column, base, self.unary())

    def primary(self):
        token = self.peek()
        if token.kind in ("int", "decimal", "str"):
            self.advance()
            return Literal(token.value)
        if token.kind == "keyword" and token.text in ("true", "false", "null"):
            self.advance()
            return Literal({"true": True, "false": False, "null": None}[token.text])
        if token.kind == "ident":
            self.advance()
            if self.at("("):
                return Call(token.text, token.column, self.arguments())
            return Name(token.text, token.column)
        if self.at("("):
            self.advance()
            inner = self.expr()
            self.expect(")")
            return inner
        self.fail()

    def arguments(self) -> tuple:
        self.expect("(")
        args = []
        if not self.at(")"):
            args.append(self.expr())
            while self.at(","):
                self.advance()
                args.append(self.expr())
        self.expect(")")
        return tuple(args)
