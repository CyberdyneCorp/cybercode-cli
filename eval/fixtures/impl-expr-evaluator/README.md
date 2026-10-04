# calc

A tiny expression language for spreadsheet-style formulas, embedded in Python 3.10+ (standard
library only).

```python
from calc import evaluate
evaluate("let total = price * qty in round(total * 1.2, 2)", {"price": Decimal("2.50"), "qty": 3})
```

`SPEC.md` is the complete language contract. The package is `calc/`: `errors.py` defines
`CalcError`; `lexer.py` is an unfinished tokenizer; `parser.py` and `evaluator.py` are stubs.

Run the tests with `python3 -m unittest`.
