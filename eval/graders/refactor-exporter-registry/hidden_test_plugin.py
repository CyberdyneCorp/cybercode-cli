"""Hidden plugin tests, run after the grader drops plugin/yaml_lines.py into reportx/exporters/.

A new format must work everywhere (library, CLI, HTTP headers, file names) without any change
outside that one module."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import reportx
from reportx.exporters import Exporter, available_formats, get_exporter, register

DATA = {"title": "Sales", "columns": ["region", "units"], "rows": [["North", 3], ["South", None]]}
YAML = 'title: "Sales"\nrows:\n  - {"region": "North", "units": 3}\n  - {"region": "South", "units": null}\n'
ALL = ["csv", "html", "json", "markdown", "ndjson", "tsv", "xml", "yaml"]
LIST_FORMATS = (
    "csv\t.csv\ttext/csv; charset=utf-8\n"
    "html\t.html\ttext/html; charset=utf-8\taliases: htm\n"
    "json\t.json\tapplication/json\n"
    "markdown\t.md\ttext/markdown; charset=utf-8\taliases: md\n"
    "ndjson\t.ndjson\tapplication/x-ndjson\taliases: jsonl\n"
    "tsv\t.tsv\ttext/tab-separated-values; charset=utf-8\n"
    "xml\t.xml\tapplication/xml\n"
    "yaml\t.yaml\tapplication/yaml\taliases: yml\n"
)


class LibraryTest(unittest.TestCase):
    def setUp(self):
        self.dataset = reportx.dataset_from_dict(DATA)

    def test_registry(self):
        self.assertEqual(available_formats(), ALL)
        self.assertIsInstance(get_exporter(" YML "), Exporter)
        self.assertEqual(type(get_exporter("yaml")).__module__, "reportx.exporters.yaml_lines")

    def test_metadata(self):
        self.assertEqual(reportx.normalize_format(" YML "), "yaml")
        self.assertEqual(reportx.extension("yml"), ".yaml")
        self.assertEqual(reportx.content_type("Yaml"), "application/yaml")
        self.assertEqual(reportx.output_filename("My Data!", "yml"), "my-data.yaml")
        self.assertEqual(reportx.guess_format("out/r.yml"), "yaml")
        self.assertEqual(reportx.guess_format("r.YAML"), "yaml")
        self.assertEqual(reportx.response_headers(self.dataset, "yml"), {
            "Content-Type": "application/yaml",
            "Content-Disposition": 'inline; filename="sales.yaml"',
            "X-Row-Count": "2",
        })

    def test_render_and_export(self):
        self.assertEqual(reportx.render(self.dataset, "yaml"), YAML)
        self.assertEqual(reportx.render(self.dataset, "YML", False), YAML)
        self.assertEqual(reportx.parse_format_list("csv, yml,yaml"), ["csv", "yaml"])
        with tempfile.TemporaryDirectory() as tmp:
            paths = reportx.export_all(self.dataset, ["yml", "csv"], tmp)
            self.assertEqual([Path(p).name for p in paths], ["sales.yaml", "sales.csv"])
            self.assertEqual((Path(tmp) / "sales.yaml").read_text(encoding="utf-8"), YAML)

    def test_unknown_format_lists_new_format(self):
        with self.assertRaises(ValueError) as caught:
            reportx.normalize_format("pdf")
        self.assertEqual(str(caught.exception), f"unknown format 'pdf' (choose from {', '.join(ALL)})")


class CliTest(unittest.TestCase):
    def run_cli(self, *args: str):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "out").mkdir()
            (work / "data.json").write_text(json.dumps(DATA))
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", PYTHONPATH=str(Path(__file__).resolve().parent))
            result = subprocess.run([sys.executable, "-m", "reportx", *args], cwd=work, env=env,
                                    capture_output=True, text=True, timeout=30)
            files = {p.name: p.read_bytes().decode("utf-8") for p in (work / "out").iterdir()}
            return result.returncode, result.stdout, result.stderr, files

    def test_list_formats(self):
        self.assertEqual(self.run_cli("--list-formats"), (0, LIST_FORMATS, "", {}))

    def test_stdout(self):
        self.assertEqual(self.run_cli("data.json", "-f", "yaml"), (0, YAML, "", {}))

    def test_guessed_from_output(self):
        self.assertEqual(self.run_cli("data.json", "-o", "out/report.yml"),
                         (0, "wrote 2 rows to out/report.yml\n", "", {"report.yml": YAML}))

    def test_no_header_warning_and_output_dir(self):
        code, out, err, files = self.run_cli("data.json", "-f", "csv,yml", "--no-header", "--output-dir", "out")
        self.assertEqual(code, 0)
        self.assertEqual(err, "reportx: warning: --no-header is ignored for yaml output\n")
        self.assertEqual(out, "wrote 2 rows to out/sales.csv\nwrote 2 rows to out/sales.yaml\n")
        self.assertEqual(files, {"sales.csv": "North,3\r\nSouth,\r\n", "sales.yaml": YAML})

    def test_unknown_format(self):
        code, out, err, _ = self.run_cli("data.json", "-f", "pdf")
        self.assertEqual((code, out), (2, ""))
        self.assertEqual(err, f"reportx: error: unknown format 'pdf' (choose from {', '.join(ALL)})\n")


class RegisterTest(unittest.TestCase):
    """Runs after LibraryTest (classes run in name order) because it registers more formats."""

    def setUp(self):
        self.dataset = reportx.dataset_from_dict(DATA)

    def test_register(self):
        class Fresh(Exporter):
            name = "fresh-format"
            extension = ".fresh"
            content_type = "text/x-fresh"

            def render(self, dataset, header):
                return "fresh\n"

        self.assertIs(register(Fresh), Fresh)
        self.assertEqual(reportx.render(self.dataset, "FRESH-FORMAT"), "fresh\n")
        for clash_name, clash_aliases in [("csv", ()), ("other1", ("md",)), ("other2", ("yml",)), ("yml", ())]:
            clash = type("Clash", (Exporter,), {"name": clash_name, "aliases": clash_aliases, "extension": ".c",
                                                "content_type": "text/plain"})
            with self.subTest(name=clash_name, aliases=clash_aliases), self.assertRaises(ValueError):
                register(clash)
        self.assertEqual(reportx.normalize_format("md"), "markdown")


if __name__ == "__main__":
    unittest.main()
