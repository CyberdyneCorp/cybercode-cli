# cache

`cache.py` has an in-memory `LRUCache(maxsize)` used to memoize lookups. Lookups of remote
data now need entries that expire, so the module needs a `TTLCache` with the same interface
plus a time-to-live (see the task description for the exact contract). `LRUCache` must keep
working unchanged.

Run the tests with `python3 -m unittest`.
