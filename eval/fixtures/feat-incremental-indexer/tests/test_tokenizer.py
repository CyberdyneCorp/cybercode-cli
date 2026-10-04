import unittest

from indexer import IndexConfig, stem, tokenize


class TokenizerTest(unittest.TestCase):
    def test_positions_skip_stopwords(self):
        tokens = list(tokenize("The quick fox and THE dog", IndexConfig(stemming=False)))
        self.assertEqual(tokens, [("quick", 1), ("fox", 2), ("dog", 5)])

    def test_stemming(self):
        self.assertEqual(stem("indexes"), "indexe")
        self.assertEqual(stem("libraries"), "library")
        self.assertEqual(stem("glass"), "glass")
        self.assertEqual(list(tokenize("Running dogs", IndexConfig())), [("runn", 0), ("dog", 1)])


if __name__ == "__main__":
    unittest.main()
