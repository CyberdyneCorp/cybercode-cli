"""Admin reports."""

from .users import UnknownUser, get_user


def active_emails(user_ids: list[int]) -> list[str]:
    """Emails of the active users among `user_ids`, skipping unknown ids."""
    emails = []
    for user_id in user_ids:
        try:
            user = get_user(user_id)
        except UnknownUser:
            continue
        if user["active"]:
            emails.append(user["email"])
    return emails
