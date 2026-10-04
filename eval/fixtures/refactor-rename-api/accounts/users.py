"""User lookup."""

from .store import USERS


class UnknownUser(LookupError):
    """Raised when no user has the requested id."""


def fetch_user_data(user_id: int) -> dict:
    """Return a copy of the user record for `user_id`."""
    try:
        return dict(USERS[user_id])
    except KeyError:
        raise UnknownUser(user_id) from None


def display_name(user_id: int) -> str:
    user = fetch_user_data(user_id)
    return f"{user['name']} <{user['email']}>"
