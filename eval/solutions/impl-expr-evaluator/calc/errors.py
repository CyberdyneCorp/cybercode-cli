"""The single exception type raised by calc."""


class CalcError(Exception):
    """A lexical, syntax or evaluation error at a 1-based column of the source."""

    def __init__(self, message: str, column: int):
        super().__init__(f"{message} at column {column}")
        self.message = message
        self.column = column
