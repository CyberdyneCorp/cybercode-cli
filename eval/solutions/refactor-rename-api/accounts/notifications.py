"""Outgoing messages."""

from . import users


def greeting(user_id: int) -> str:
    first_name = users.get_user(user_id)["name"].split()[0]
    return f"Hello, {first_name}!"
