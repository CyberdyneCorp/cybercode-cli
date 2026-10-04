"""Small in-memory caches."""

import time
from collections import OrderedDict
from typing import Any, Callable, Hashable


class LRUCache:
    """Keeps at most `maxsize` entries, evicting the least recently used one."""

    def __init__(self, maxsize: int = 128) -> None:
        if maxsize < 1:
            raise ValueError("maxsize must be at least 1")
        self.maxsize = maxsize
        self._data: OrderedDict[Hashable, Any] = OrderedDict()

    def get(self, key: Hashable, default: Any = None) -> Any:
        """Return the cached value (marking it most recently used) or `default`."""
        if key not in self._data:
            return default
        self._data.move_to_end(key)
        return self._data[key]

    def set(self, key: Hashable, value: Any) -> None:
        """Store `value`, marking it most recently used, and evict if over capacity."""
        self._data[key] = value
        self._data.move_to_end(key)
        while len(self._data) > self.maxsize:
            self._data.popitem(last=False)

    def __contains__(self, key: Hashable) -> bool:
        return key in self._data

    def __len__(self) -> int:
        return len(self._data)


class TTLCache:
    """An LRU cache whose entries expire `ttl` seconds after they were set."""

    def __init__(self, maxsize: int, ttl: float, clock: Callable[[], float] = time.monotonic) -> None:
        if maxsize < 1:
            raise ValueError("maxsize must be at least 1")
        if ttl <= 0:
            raise ValueError("ttl must be positive")
        self.maxsize = maxsize
        self.ttl = ttl
        self._clock = clock
        self._data: OrderedDict[Hashable, tuple[float, Any]] = OrderedDict()

    def _purge(self) -> None:
        now = self._clock()
        for key in [k for k, (expires, _) in self._data.items() if expires <= now]:
            del self._data[key]

    def get(self, key: Hashable, default: Any = None) -> Any:
        """Return the live value (marking it most recently used) or `default`."""
        self._purge()
        if key not in self._data:
            return default
        self._data.move_to_end(key)
        return self._data[key][1]

    def set(self, key: Hashable, value: Any) -> None:
        """Store `value` for `ttl` seconds, marking it most recently used."""
        self._purge()
        self._data[key] = (self._clock() + self.ttl, value)
        self._data.move_to_end(key)
        while len(self._data) > self.maxsize:
            self._data.popitem(last=False)

    def __contains__(self, key: Hashable) -> bool:
        self._purge()
        return key in self._data

    def __len__(self) -> int:
        self._purge()
        return len(self._data)
