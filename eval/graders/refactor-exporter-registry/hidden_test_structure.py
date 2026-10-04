"""Hidden structural tests: the exporter registry API and where format knowledge may live."""

import ast
import unittest
from pathlib import Path

import reportx.exporters as exporters
import reportx.exporters.base as base

ROOT = Path(exporters.__file__).resolve().parent.parent  # the reportx package directory
# format -> (aliases, extension, content type, disposition, header_row)
BUILTINS = {
    "csv": ((), ".csv", "text/csv; charset=utf-8", "attachment", True),
    "tsv": ((), ".tsv", "text/tab-separated-values; charset=utf-8", "attachment", True),
    "json": ((), ".json", "application/json", "attachment", False),
    "ndjson": (("jsonl",), ".ndjson", "application/x-ndjson", "attachment", False),
    "markdown": (("md",), ".md", "text/markdown; charset=utf-8", "attachment", True),
    "html": (("htm",), ".html", "text/html; charset=utf-8", "inline", True),
    "xml": ((), ".xml", "application/xml", "attachment", False),
}


def owned_literals() -> dict[str, str]:
    """Every format-specific literal mapped to the format that owns it."""
    owners = {}
    for name, (aliases, ext, ctype, _, _) in BUILTINS.items():
        for literal in (name, *aliases, ext, ctype):
            owners[literal] = name
    return owners


def string_literals(path: Path) -> set[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"))
    return {node.value for node in ast.walk(tree) if isinstance(node, ast.Constant) and isinstance(node.value, str)}


class RegistryApiTest(unittest.TestCase):
    def test_api_names(self):
        for name in ("Exporter", "register", "get_exporter", "available_formats"):
            self.assertTrue(hasattr(base, name), f"reportx.exporters.base.{name} is missing")
            self.assertIs(getattr(exporters, name), getattr(base, name), f"reportx.exporters.{name}")
        self.assertIsInstance(base.Exporter, type)

    def test_available_formats(self):
        self.assertEqual(exporters.available_formats(), sorted(BUILTINS))

    def test_builtin_exporters(self):
        modules = {}
        for name, (aliases, ext, ctype, disposition, header_row) in BUILTINS.items():
            with self.subTest(fmt=name):
                exporter = exporters.get_exporter(name)
                self.assertIsInstance(exporter, base.Exporter)
                self.assertEqual((exporter.name, tuple(exporter.aliases), exporter.extension, exporter.content_type,
                                  exporter.disposition, exporter.header_row),
                                 (name, aliases, ext, ctype, disposition, header_row))
                module = type(exporter).__module__
                self.assertTrue(module.startswith("reportx.exporters.") and module != "reportx.exporters.base",
                                f"{name} exporter is defined in {module}")
                modules[name] = module
                for alias in aliases:
                    self.assertIs(exporters.get_exporter(f" {alias.upper()} "), exporter)
        self.assertEqual(len(set(modules.values())), len(BUILTINS), f"formats must have separate modules: {modules}")

    def test_unknown_format_message(self):
        with self.assertRaises(ValueError) as caught:
            exporters.get_exporter("Pdf")
        self.assertEqual(str(caught.exception),
                         "unknown format 'Pdf' (choose from csv, html, json, markdown, ndjson, tsv, xml)")


class FormatLiteralTest(unittest.TestCase):
    def test_format_literals_only_in_their_exporter_module(self):
        owners = owned_literals()
        module_of = {name: type(exporters.get_exporter(name)).__module__ for name in BUILTINS}
        for path in sorted(ROOT.rglob("*.py")):
            if "__pycache__" in path.parts:
                continue
            module = "reportx." + ".".join(path.relative_to(ROOT).with_suffix("").parts)
            for literal in sorted(string_literals(path) & owners.keys()):
                owner = owners[literal]
                with self.subTest(file=str(path.relative_to(ROOT.parent)), literal=literal):
                    self.assertEqual(module, module_of[owner],
                                     f"{literal!r} (format {owner}) may only appear in {module_of[owner]}")


if __name__ == "__main__":
    unittest.main()
