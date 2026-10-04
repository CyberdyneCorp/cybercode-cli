import unittest

from indexer import Document, IndexConfig, InvertedIndex, QuerySyntaxError, search
from indexer.tokenizer import group_positions, tokenize

CONFIG = IndexConfig()


def make_index(texts: dict[str, str]) -> InvertedIndex:
    index = InvertedIndex()
    for path in sorted(texts):
        terms = group_positions(tokenize(texts[path], CONFIG))
        index.add_document(Document(path, 0, 0, "", sum(map(len, terms.values()))), terms)
    return index


class QueryTest(unittest.TestCase):
    def setUp(self):
        self.index = make_index({
            "a.txt": "state of the art parsers",
            "b.txt": "the art of the state",
            "c.txt": "parsers for state machines",
        })

    def test_and_or(self):
        self.assertEqual(search(self.index, "state parsers", CONFIG), ["a.txt", "c.txt"])
        self.assertEqual(search(self.index, "machines OR art", CONFIG), ["a.txt", "b.txt", "c.txt"])
        self.assertEqual(search(self.index, "(machines OR art) AND parsers", CONFIG), ["a.txt", "c.txt"])

    def test_phrase_respects_stopword_gaps(self):
        self.assertEqual(search(self.index, '"state of the art"', CONFIG), ["a.txt"])
        self.assertEqual(search(self.index, '"state art"', CONFIG), [])

    def test_syntax_errors(self):
        for query in ["", "(state", '"open', "state OR"]:
            with self.subTest(query=query), self.assertRaises(QuerySyntaxError):
                search(self.index, query, CONFIG)


if __name__ == "__main__":
    unittest.main()
