"""Tokenizer for calc (work in progress: see SPEC.md, Lexical structure)."""

import re
from dataclasses import dataclass
from decimal import Decimal

from .errors import CalcError

KEYWORDS = {"true", "false", "null", "and", "or", "not", "if", "else", "let"}
OPERATORS = ("+", "-", "*", "/", "%", "<", ">", "(", ")", ",", "=", "**", "//", "==", "!=", "<=", ">=")
ESCAPES = {"n": "\n", "t": "\t", "\\": "\\", '"': '"'}

NUMBER = re.compile(r"[0-9]+(\.[0-9]+)?")
IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


@dataclass(frozen=True)
class Token:
    kind: str  # "int", "decimal", "str", "ident", "keyword", "op" or "eof"
    text: str
    column: int
    value: object = None


def tokenize(source: str) -> list[Token]:
    tokens = []
    pos = 0
    while pos < len(source):
        char = source[pos]
        if char.isspace():
            pos += 1
            continue
        if char.isdigit():
            text = NUMBER.match(source, pos).group()
            value = Decimal(text) if "." in text else int(text)
            tokens.append(Token("decimal" if "." in text else "int", text, pos, value))
            pos += len(text)
        elif char == '"':
            end = pos + 1
            chars = []
            while end < len(source) and source[end] != '"':
                if source[end] == "\\" and end + 1 < len(source):
                    chars.append(ESCAPES.get(source[end + 1], source[end + 1]))
                    end += 2
                else:
                    chars.append(source[end])
                    end += 1
            if end >= len(source):
                raise CalcError("unterminated string", pos)
            tokens.append(Token("str", source[pos:end + 1], pos, "".join(chars)))
            pos = end + 1
        elif IDENT.match(source, pos):
            text = IDENT.match(source, pos).group()
            tokens.append(Token("keyword" if text in KEYWORDS else "ident", text, pos))
            pos += len(text)
        else:
            for op in OPERATORS:
                if source.startswith(op, pos):
                    tokens.append(Token("op", op, pos))
                    pos += len(op)
                    break
            else:
                raise CalcError(f"unexpected character '{char}'", pos)
    tokens.append(Token("eof", "", pos))
    return tokens
