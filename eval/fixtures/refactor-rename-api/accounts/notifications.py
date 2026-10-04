"""Outgoing messages."""

from . import users


def greeting(user_id: int) -> str:
    first_name = users.fetch_user_data(user_id)["name"].split()[0]
    return f"Hello, {first_name}!"
