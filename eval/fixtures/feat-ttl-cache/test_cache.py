import unittest

from cache import LRUCache


class LRUCacheTest(unittest.TestCase):
    def test_get_returns_default(self):
        cache = LRUCache(2)
        self.assertIsNone(cache.get("missing"))
        self.assertEqual(cache.get("missing", 0), 0)

    def test_evicts_least_recently_used(self):
        cache = LRUCache(2)
        cache.set("a", 1)
        cache.set("b", 2)
        cache.get("a")
        cache.set("c", 3)
        self.assertIn("a", cache)
        self.assertNotIn("b", cache)
        self.assertEqual(len(cache), 2)


if __name__ == "__main__":
    unittest.main()
