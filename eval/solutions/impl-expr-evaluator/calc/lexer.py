"""Tokenizer: turns source text into a list of tokens (see SPEC.md, Lexical structure)."""

import re
from dataclasses import dataclass
from decimal import Decimal

from .errors import CalcError

KEYWORDS = frozenset({"true", "false", "null", "and", "or", "not", "if", "else", "let", "in"})
OPERATORS = ("**", "//", "==", "!=", "<=", ">=", "+", "-", "*", "/", "%", "<", ">", "(", ")", ",", "=")
WHITESPACE = " \t\n\r"
SIMPLE_ESCAPES = {"\\": "\\", '"': '"', "'": "'", "n": "\n", "t": "\t", "r": "\r"}

_DIGITS = r"[0-9](?:_?[0-9])*"
_NUMBER = re.compile(rf"(?:0|[1-9](?:_?[0-9])*)(\.{_DIGITS})?")
_NUMBER_RUN = re.compile(r"[A-Za-z0-9_.]*")
_IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_UNICODE_ESCAPE = re.compile(r"\\u\{([0-9A-Fa-f]{1,6})\}")


@dataclass(frozen=True)
class Token:
    kind: str  # "int", "decimal", "str", "ident", "keyword", "op" or "eof"
    text: str  # exact source text
    column: int  # 1-based
    value: object = None  # literal value for int/decimal/str tokens


def tokenize(source: str) -> list[Token]:
    """Return all tokens of `source`, ending with an "eof" token at column len(source) + 1."""
    tokens = []
    pos = 0
    while pos < len(source):
        char = source[pos]
        if char in WHITESPACE:
            pos += 1
            continue
        if char.isascii() and char.isdigit():
            token = _number(source, pos)
        elif char in "\"'":
            token = _string(source, pos)
        elif _IDENT.match(source, pos):
            text = _IDENT.match(source, pos).group()
            token = Token("keyword" if text in KEYWORDS else "ident", text, pos + 1)
        else:
            token = _operator(source, pos)
        tokens.append(token)
        pos += len(token.text)
    tokens.append(Token("eof", "", len(source) + 1))
    return tokens


def _number(source: str, pos: int) -> Token:
    text = _NUMBER_RUN.match(source, pos).group()
    match = _NUMBER.fullmatch(text)
    if not match:
        raise CalcError(f"invalid number literal '{text}'", pos + 1)
    plain = text.replace("_", "")
    if match.group(1):
        return Token("decimal", text, pos + 1, Decimal(plain))
    return Token("int", text, pos + 1, int(plain))


def _string(source: str, start: int) -> Token:
    quote = source[start]
    chars = []
    pos = start + 1
    while True:
        char = source[pos] if pos < len(source) else ""
        if char in ("", "\n") or (char == "\\" and source[pos + 1:pos + 2] in ("", "\n")):
            raise CalcError("unterminated string", start + 1)
        if char == quote:
            return Token("str", source[start:pos + 1], start + 1, "".join(chars))
        if char == "\\":
            char, length = _escape(source, pos)
        else:
            length = 1
        chars.append(char)
        pos += length


def _escape(source: str, pos: int) -> tuple[str, int]:
    """Decode the escape sequence whose backslash is at `pos`; return (char, source length)."""
    follower = source[pos + 1]
    if follower in SIMPLE_ESCAPES:
        return SIMPLE_ESCAPES[follower], 2
    if follower != "u":
        raise CalcError(f"invalid escape sequence '\\{follower}'", pos + 1)
    match = _UNICODE_ESCAPE.match(source, pos)
    code = int(match.group(1), 16) if match else -1
    if not 0 <= code <= 0x10FFFF or 0xD800 <= code <= 0xDFFF:
        raise CalcError("invalid unicode escape", pos + 1)
    return chr(code), len(match.group())


def _operator(source: str, pos: int) -> Token:
    for op in OPERATORS:
        if source.startswith(op, pos):
            return Token("op", op, pos + 1)
    raise CalcError(f"unexpected character '{source[pos]}'", pos + 1)
