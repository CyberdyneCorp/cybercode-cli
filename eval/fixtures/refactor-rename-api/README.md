# accounts

A small account-service package. `accounts.users.fetch_user_data(user_id)` loads a user
record; billing, notifications and reports build on it. The team is renaming it to
`get_user` across the code base, but external callers still import `fetch_user_data`, so the
old name must keep working for one release as a deprecated alias.

Run the tests with `python3 -m unittest`.
