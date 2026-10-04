"""Hidden differential tests: reportx must behave byte for byte like the original package
(frozen in original_reportx/) for every library function and CLI invocation."""

import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import original_reportx as original
import reportx

SEED = 4242
HERE = Path(__file__).resolve().parent
FORMAT_NAMES = ["csv", "tsv", "json", "ndjson", "markdown", "html", "xml", "md", "jsonl", "htm",
                " CSV ", "Md", "HTML", "Xml", "NDJSON", "JsonL", "pdf", "", "c sv", "markdown,", "yaml"]
STRINGS = ["", " ", "plain", "a,b", 'say "hi"', "line1\nline2", "crlf\r\nend", "cr\ronly", "tab\there",
           "pipe | bar", "back\\slash", "<b>&amp;</b>", "'single'", "  padded  ", "naïve café", "emoji 🎉",
           "ctrl\x01char", "\\N", "-", "#", "_under_", "x" * 30, "ünïcödé|<&>\"'", "=1+2", " sep"]
TITLES = ["", "Sales", "Q3 sales | by region", "  spaced\n title  ", "<Report> & \"Co\"", "ÄÖÜ 2024",
          "---", "a/b\\c", "tab\ttitle", "emoji 🎉 report"]
COLUMNS = ["id", "name", "a b", "x|y", "<c>", "naïve", 'q"uote', "tab\tcol", "units", "price", "note",
           "Total", "#", "line\nbreak"]


def random_cell(rng: random.Random):
    roll = rng.random()
    if roll < 0.35:
        return rng.choice(STRINGS)
    if roll < 0.55:
        return rng.choice([0, 1, -7, 42, 10 ** 20, 3])
    if roll < 0.7:
        return rng.choice([0.0, 1.5, -2.25, 1e-07, 3.14159, 1e22, 100.0])
    if roll < 0.8:
        return rng.choice([True, False])
    return None


def random_data(rng: random.Random) -> dict:
    columns = rng.sample(COLUMNS, rng.randint(1, 5))
    kind = rng.random()
    rows = []
    for _ in range(rng.choice([0, 1, 1, 2, 3, 5, 8])):
        if kind < 0.25:  # numeric-only columns exercise markdown alignment
            rows.append([rng.choice([None, 1, 2.5, -3]) for _ in columns])
        else:
            rows.append([random_cell(rng) for _ in columns])
    return {"title": rng.choice(TITLES), "columns": columns, "rows": rows}


def outcome(function, *args):
    try:
        return ("ok", function(*args))
    except ValueError as error:
        return ("ValueError", str(error))


class LibraryDifferentialTest(unittest.TestCase):
    def datasets(self, count=150):
        rng = random.Random(SEED)
        for number in range(count):
            data = random_data(rng)
            yield number, data, reportx.dataset_from_dict(data), original.dataset_from_dict(data)

    def test_render(self):
        for number, data, mine, theirs in self.datasets():
            for name in FORMAT_NAMES:
                for header in (True, False):
                    with self.subTest(case=number, fmt=name, header=header, data=data):
                        self.assertEqual(outcome(reportx.render, mine, name, header),
                                         outcome(original.render, theirs, name, header))

    def test_render_default_header(self):
        for number, data, mine, theirs in self.datasets(20):
            for name in FORMAT_NAMES[:10]:
                with self.subTest(case=number, fmt=name):
                    self.assertEqual(outcome(reportx.render, mine, name), outcome(original.render, theirs, name))

    def test_metadata(self):
        for name in FORMAT_NAMES:
            with self.subTest(fmt=name):
                for function in ("content_type", "extension", "normalize_format"):
                    self.assertEqual(outcome(getattr(reportx, function), name),
                                     outcome(getattr(original, function), name), function)
                for title in TITLES:
                    self.assertEqual(outcome(reportx.output_filename, title, name),
                                     outcome(original.output_filename, title, name))

    def test_response_headers(self):
        for number, data, mine, theirs in self.datasets(30):
            for name in FORMAT_NAMES:
                with self.subTest(case=number, fmt=name):
                    self.assertEqual(outcome(reportx.response_headers, mine, name),
                                     outcome(original.response_headers, theirs, name))

    def test_guess_format(self):
        paths = ["r.csv", "r.CSV", "r.tsv", "r.json", "r.JSONL", "r.ndjson", "r.md", "r.markdown", "r.Markdown",
                 "r.html", "r.htm", "r.HTM", "r.xml", "r.txt", "r", "dir.csv/r", "r.csv.bak", ".csv", "r.yaml",
                 "out/r.jsonl", "r.tab", "r.mdx", "r.xhtml", "r."]
        for path in paths:
            with self.subTest(path=path):
                self.assertEqual(reportx.guess_format(path), original.guess_format(path))

    def test_parse_format_list(self):
        texts = ["csv", "csv,json", " md , markdown,MD ", "csv,,tsv", ",", "", "xml,pdf", "jsonl,ndjson,json",
                 "html, htm ,HTML,xml", "tsv,csv,tsv"]
        for text in texts:
            with self.subTest(text=text):
                self.assertEqual(outcome(reportx.parse_format_list, text), outcome(original.parse_format_list, text))

    def test_export_all(self):
        lists = [["csv"], ["md", "html"], ["xml", "jsonl", "tsv"], ["markdown", "csv"], ["csv", "pdf"]]
        for number, data, mine, theirs in self.datasets(25):
            for formats in lists:
                for header in (True, False):
                    with self.subTest(case=number, formats=formats, header=header), \
                            tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
                        result_a = outcome(reportx.export_all, mine, formats, a, header)
                        result_b = outcome(original.export_all, theirs, formats, b, header)
                        if result_a[0] == "ok":
                            result_a = ("ok", [Path(p).relative_to(a).as_posix() for p in result_a[1]])
                        if result_b[0] == "ok":
                            result_b = ("ok", [Path(p).relative_to(b).as_posix() for p in result_b[1]])
                        self.assertEqual(result_a, result_b)
                        self.assertEqual(snapshot(Path(a)), snapshot(Path(b)))


def snapshot(directory: Path) -> dict:
    return {p.relative_to(directory).as_posix(): p.read_bytes() for p in sorted(directory.rglob("*")) if p.is_file()}


CLI_ARGS = [
    ["--list-formats"],
    ["{data}", "-f", "csv"], ["{data}", "--format", "TSV"], ["{data}", "-f", "json"], ["{data}", "-f", "jsonl"],
    ["{data}", "-f", "md"], ["{data}", "-f", "html"], ["{data}", "-f", "xml"], ["{data}", "-f", "pdf"],
    ["{data}", "-f", "csv", "--no-header"], ["{data}", "-f", "markdown", "--no-header"],
    ["{data}", "-f", "ndjson", "--no-header"], ["{data}", "-f", "xml,json,csv", "--no-header", "--output-dir", "out"],
    ["{data}", "-f", "csv,md,html,jsonl", "--output-dir", "out"], ["{data}", "-f", "csv,tsv"],
    ["{data}", "-f", "md,csv", "--no-header", "--output-dir", "out"],
    ["{data}", "-o", "out/report.HTM"], ["{data}", "-o", "out/report.jsonl"], ["{data}", "-o", "out/report.txt"],
    ["{data}", "-f", "xml", "-o", "out/report.csv"], ["{data}", "-f", "json", "--no-header", "-o", "out/r.json"],
    ["{data}"], [], ["missing.json", "-f", "csv"], ["{data}", "-f", "csv", "--output-dir", "nowhere"],
    ["{data}", "-f", ",", "--output-dir", "out"],
]
CLI_DATA = [
    {"title": "Q3 sales | by region", "columns": ["region", "units", "revenue", "note"],
     "rows": [["North", 120, 1520.5, 'best "quarter"'], ["South, coastal", 80, 990.25, None],
              ["West", 0, 0.0, "line one\nline two"], ["<East>", 7, 70.0, ""]]},
    {"title": "", "columns": ["only"], "rows": []},
    {"title": "One", "columns": ["a", "b"], "rows": [[True, "naïve 🎉"]]},
]
BAD_DATA = ['{"title": 1, "columns": ["a"]}', '{"columns": []}', '{"columns": ["a", "a"]}',
            '{"columns": ["a"], "rows": [[1, 2]]}', "not json", '{"columns": ["a"], "rows": [[[1]]]}']


class CliDifferentialTest(unittest.TestCase):
    def run_cli(self, package: str, args: list[str], data: str):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "out").mkdir()
            (work / "data.json").write_text(data, encoding="utf-8")
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", PYTHONPATH=str(HERE))
            argv = [arg.replace("{data}", "data.json") for arg in args]
            result = subprocess.run([sys.executable, "-m", package, *argv], cwd=work, env=env,
                                    capture_output=True, timeout=30)
            return result.returncode, result.stdout, result.stderr, snapshot(work / "out")

    def test_cli(self):
        cases = [(json.dumps(data), args) for data in CLI_DATA for args in CLI_ARGS[1:]]
        cases += [("{}", CLI_ARGS[0])] + [(data, ["{data}", "-f", "csv"]) for data in BAD_DATA]
        for data, args in cases:
            with self.subTest(data=data[:40], args=args):
                self.assertEqual(self.run_cli("reportx", args, data), self.run_cli("original_reportx", args, data))


if __name__ == "__main__":
    unittest.main()
