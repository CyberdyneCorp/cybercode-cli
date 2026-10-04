import unittest

from cache import LRUCache, TTLCache


class FakeClock:
    def __init__(self, now=1000.0):
        self.now = now

    def __call__(self):
        return self.now


def make(maxsize=3, ttl=10):
    clock = FakeClock()
    return TTLCache(maxsize, ttl, clock=clock), clock


class HiddenLRUCacheTest(unittest.TestCase):
    def test_lru_unchanged(self):
        cache = LRUCache(2)
        cache.set("a", 1)
        cache.set("b", 2)
        cache.get("a")
        cache.set("c", 3)
        self.assertEqual((cache.get("a"), cache.get("b"), cache.get("c")), (1, None, 3))
        self.assertEqual(cache.get("missing", 0), 0)
        self.assertRaises(ValueError, LRUCache, 0)


class HiddenTTLCacheTest(unittest.TestCase):
    def test_get_and_default(self):
        cache, _ = make()
        cache.set("a", 1)
        self.assertEqual(cache.get("a"), 1)
        self.assertIsNone(cache.get("b"))
        self.assertEqual(cache.get("b", "dflt"), "dflt")

    def test_falsy_values_are_cached(self):
        cache, _ = make()
        cache.set("zero", 0)
        cache.set("none", None)
        self.assertEqual(cache.get("zero", "dflt"), 0)
        self.assertIsNone(cache.get("none", "dflt"))
        self.assertIn("none", cache)

    def test_expires_exactly_at_ttl(self):
        cache, clock = make(ttl=10)
        cache.set("a", 1)
        clock.now += 9.999
        self.assertEqual(cache.get("a"), 1)
        self.assertIn("a", cache)
        clock.now += 0.001
        self.assertNotIn("a", cache)
        self.assertEqual(cache.get("a", "gone"), "gone")

    def test_len_counts_only_live_entries(self):
        cache, clock = make(maxsize=5, ttl=10)
        cache.set("a", 1)
        clock.now += 5
        cache.set("b", 2)
        self.assertEqual(len(cache), 2)
        clock.now += 5
        self.assertEqual(len(cache), 1)
        clock.now += 5
        self.assertEqual(len(cache), 0)

    def test_get_does_not_extend_expiry(self):
        cache, clock = make(ttl=10)
        cache.set("a", 1)
        for _ in range(3):
            clock.now += 3
            self.assertEqual(cache.get("a"), 1)
        clock.now += 1
        self.assertIsNone(cache.get("a"))

    def test_set_resets_expiry_and_replaces_value(self):
        cache, clock = make(ttl=10)
        cache.set("a", 1)
        clock.now += 8
        cache.set("a", 2)
        clock.now += 8
        self.assertEqual(cache.get("a"), 2)
        self.assertEqual(len(cache), 1)

    def test_evicts_least_recently_used(self):
        cache, clock = make(maxsize=2, ttl=100)
        cache.set("a", 1)
        clock.now += 1
        cache.set("b", 2)
        clock.now += 1
        self.assertEqual(cache.get("a"), 1)
        cache.set("c", 3)
        self.assertIn("a", cache)
        self.assertNotIn("b", cache)
        self.assertIn("c", cache)
        self.assertEqual(len(cache), 2)

    def test_contains_does_not_refresh_recency(self):
        cache, _ = make(maxsize=2, ttl=100)
        cache.set("a", 1)
        cache.set("b", 2)
        self.assertIn("a", cache)
        cache.set("c", 3)
        self.assertNotIn("a", cache)
        self.assertIn("b", cache)

    def test_expired_entries_are_dropped_before_live_ones(self):
        cache, clock = make(maxsize=2, ttl=10)
        cache.set("a", 1)
        clock.now += 1
        cache.set("b", 2)
        clock.now += 1
        cache.get("a")  # a is now most recently used, b least
        clock.now += 8.5  # a expired (set at +0), b still live (expires at +11)
        cache.set("c", 3)
        self.assertEqual(cache.get("b"), 2)
        self.assertEqual(cache.get("c"), 3)
        self.assertNotIn("a", cache)
        self.assertEqual(len(cache), 2)

    def test_capacity_never_exceeded(self):
        cache, clock = make(maxsize=3, ttl=50)
        for i in range(20):
            cache.set(i, i * i)
            clock.now += 1
            self.assertLessEqual(len(cache), 3)
        self.assertEqual([k for k in range(20) if k in cache], [17, 18, 19])

    def test_validation(self):
        clock = FakeClock()
        for maxsize, ttl in [(0, 10), (-1, 10), (3, 0), (3, -5)]:
            with self.subTest(maxsize=maxsize, ttl=ttl):
                with self.assertRaises(ValueError):
                    TTLCache(maxsize, ttl, clock=clock)
        TTLCache(1, 0.5, clock=clock)

    def test_default_clock_is_monotonic(self):
        cache = TTLCache(2, 60)
        cache.set("k", "v")
        self.assertEqual(cache.get("k"), "v")
        self.assertEqual(len(cache), 1)


if __name__ == "__main__":
    unittest.main()
