import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from reportx import content_type, dataset_from_dict, extension, guess_format, normalize_format, render

DATA = {"title": "Sales", "columns": ["region", "units"], "rows": [["North, east", 3], ["South", None]]}


class RenderTest(unittest.TestCase):
    def test_csv(self):
        text = render(dataset_from_dict(DATA), "csv")
        self.assertEqual(text, 'region,units\r\n"North, east",3\r\nSouth,\r\n')

    def test_markdown_alias(self):
        text = render(dataset_from_dict(DATA), "MD")
        self.assertTrue(text.startswith("# Sales\n\n| region | units |\n| --- | ---: |\n"))
        self.assertTrue(text.endswith("\n_2 rows_\n"))

    def test_ndjson_has_no_trailing_newline(self):
        text = render(dataset_from_dict(DATA), "jsonl")
        self.assertEqual(text.splitlines()[1], '{"region":"South","units":null}')
        self.assertFalse(text.endswith("\n"))


class MetadataTest(unittest.TestCase):
    def test_metadata(self):
        self.assertEqual(content_type("html"), "text/html; charset=utf-8")
        self.assertEqual(extension("markdown"), ".md")
        self.assertEqual(guess_format("out/report.HTM"), "html")
        self.assertIsNone(guess_format("report.txt"))
        with self.assertRaises(ValueError):
            normalize_format("pdf")


class CliTest(unittest.TestCase):
    def test_writes_into_output_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "data.json"
            source.write_text(json.dumps(DATA))
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
            result = subprocess.run([sys.executable, "-m", "reportx", str(source), "-f", "tsv,xml", "--output-dir", tmp],
                                    capture_output=True, text=True, env=env, cwd=Path(__file__).parent)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((Path(tmp) / "sales.tsv").read_text(), "region\tunits\nNorth, east\t3\nSouth\t\\N\n")
            self.assertTrue((Path(tmp) / "sales.xml").exists())


if __name__ == "__main__":
    unittest.main()
