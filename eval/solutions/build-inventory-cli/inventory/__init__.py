"""Command-line inventory tracker (see SPEC.md)."""


class InventoryError(Exception):
    """An error reported as `error: <message>` with a specific exit code."""

    def __init__(self, message: str, exit_code: int):
        super().__init__(message)
        self.exit_code = exit_code


def invalid(field: str, value: str) -> InventoryError:
    return InventoryError(f"invalid {field}: {value}", 2)


def not_found(sku: str) -> InventoryError:
    return InventoryError(f"sku {sku} not found", 1)
