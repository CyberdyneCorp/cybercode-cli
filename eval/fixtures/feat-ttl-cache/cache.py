"""Small in-memory caches."""

from collections import OrderedDict
from typing import Any, Hashable


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
