"""Module docstring."""
MAX_RETRIES = 3
lower_value = 1


class Session(Base):
    """Session docstring that must not appear."""

    def refresh(self, token):
        """Method docstring."""
        return helper(token)


def helper(value):  # comment
    return value
