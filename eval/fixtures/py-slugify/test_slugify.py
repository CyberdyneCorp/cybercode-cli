import unittest

from slugify import slugify


class SlugifyTest(unittest.TestCase):
    def test_simple(self):
        self.assertEqual(slugify("Hello"), "hello")


if __name__ == "__main__":
    unittest.main()
