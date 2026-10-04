import unittest

from slugify import slugify

CASES = {
    "Hello": "hello",
    "Hello,  World!": "hello-world",
    "  leading and trailing  ": "leading-and-trailing",
    "Rust 2024 Edition": "rust-2024-edition",
    "---": "",
    "": "",
    "a--b__c": "a-b-c",
    "Ünïcode Títle": "ünïcode-títle",
}


class HiddenSlugifyTest(unittest.TestCase):
    def test_cases(self):
        for given, expected in CASES.items():
            with self.subTest(given=given):
                self.assertEqual(slugify(given), expected)


if __name__ == "__main__":
    unittest.main()
