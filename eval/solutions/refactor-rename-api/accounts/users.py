"""User lookup."""

import warnings

from .store import USERS


class UnknownUser(LookupError):
    """Raised when no user has the requested id."""


def get_user(user_id: int) -> dict:
    """Return a copy of the user record for `user_id`."""
    try:
        return dict(USERS[user_id])
    except KeyError:
        raise UnknownUser(user_id) from None


def fetch_user_data(user_id: int) -> dict:
    """Deprecated alias of `get_user`."""
    warnings.warn("fetch_user_data is deprecated; use get_user", DeprecationWarning, stacklevel=2)
    return get_user(user_id)


def display_name(user_id: int) -> str:
    user = get_user(user_id)
    return f"{user['name']} <{user['email']}>"
