"""calc: a tiny expression language. SPEC.md is the contract."""

from .errors import CalcError
from .evaluator import evaluate

__all__ = ["CalcError", "evaluate"]
