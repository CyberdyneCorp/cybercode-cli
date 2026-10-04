"""Evaluator for calc (see SPEC.md, Evaluation). Not implemented yet."""

from .parser import parse


def evaluate(source: str, env: dict | None = None):
    """Evaluate one calc expression; see SPEC.md for the full contract."""
    tree = parse(source)
    raise NotImplementedError("evaluator not implemented")
