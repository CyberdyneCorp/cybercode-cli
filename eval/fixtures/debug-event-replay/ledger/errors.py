"""Exception types raised by the ledger."""


class LedgerError(Exception):
    """Base class for ledger errors."""


class InvalidAmount(LedgerError, ValueError):
    """An amount is not a positive decimal with at most two fractional digits."""


class UnknownWallet(LedgerError, KeyError):
    """No wallet with this id has been opened."""

    def __str__(self) -> str:
        return f"unknown wallet: {self.args[0]}"


class WalletExists(LedgerError):
    """A wallet with this id is already open."""


class InsufficientFunds(LedgerError):
    """A withdrawal exceeds the wallet balance."""


class CorruptLog(LedgerError):
    """The event log or a snapshot cannot be decoded."""
