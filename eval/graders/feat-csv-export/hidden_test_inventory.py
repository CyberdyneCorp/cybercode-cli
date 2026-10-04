import csv
import io
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from inventory import Item, export_csv, format_table, load_items

HERE = Path(__file__).resolve().parent
SAMPLE_JSON = r"""[
  {"sku": "MUG-01", "name": "Mug, large", "quantity": 12, "price": 7.5, "discontinued": false, "supplier": "Acme"},
  {"sku": "TEA-02", "name": "Teapot \"Classic\"", "quantity": 0, "price": 24.0, "discontinued": true},
  {"sku": "SPN-03", "name": "Spoon", "quantity": 140, "price": 1.25, "discontinued": false, "supplier": "Forks & Co"}
]
"""
_SAMPLE_DIR = tempfile.TemporaryDirectory()
SAMPLE = Path(_SAMPLE_DIR.name) / "items.json"  # the grader's own copy, not the workspace's
SAMPLE.write_text(SAMPLE_JSON, encoding="utf-8")
HEADER = "sku,name,quantity,price,discontinued,supplier\n"
SAMPLE_CSV = (
    HEADER
    + "MUG-01,\"Mug, large\",12,7.50,no,Acme\n"
    + "TEA-02,\"Teapot \"\"Classic\"\"\",0,24.00,yes,\n"
    + "SPN-03,Spoon,140,1.25,no,Forks & Co\n"
)


def items():
    return [
        Item("A-1", "Plain", 3, 2.0, False, "S"),
        Item("B-2", "Comma, inside", 0, 0.125, True, None),
        Item("C-3", 'Quote "q"', 10, 1234.5, False, "Line\nbreak"),
    ]


class HiddenExportTest(unittest.TestCase):
    def test_visible_behaviour_kept(self):
        self.assertEqual(format_table(load_items(SAMPLE)[:1]),
                         "SKU     NAME        QTY  PRICE\nMUG-01  Mug, large  12   7.50\n")

    def test_exact_bytes_to_file_object(self):
        buffer = io.StringIO()
        self.assertEqual(export_csv(load_items(SAMPLE), buffer), 3)
        self.assertEqual(buffer.getvalue(), SAMPLE_CSV)
        self.assertFalse(buffer.closed)

    def test_round_trip_with_csv_reader(self):
        buffer = io.StringIO()
        export_csv(items(), buffer)
        rows = list(csv.reader(io.StringIO(buffer.getvalue())))
        self.assertEqual(rows, [
            ["sku", "name", "quantity", "price", "discontinued", "supplier"],
            ["A-1", "Plain", "3", "2.00", "no", "S"],
            ["B-2", "Comma, inside", "0", "0.12", "yes", ""],
            ["C-3", 'Quote "q"', "10", "1234.50", "no", "Line\nbreak"],
        ])

    def test_columns_select_and_order(self):
        buffer = io.StringIO()
        export_csv(items(), buffer, columns=["price", "sku", "discontinued"])
        self.assertEqual(buffer.getvalue(), "price,sku,discontinued\n2.00,A-1,no\n0.12,B-2,yes\n1234.50,C-3,no\n")

    def test_empty_records_write_header(self):
        buffer = io.StringIO()
        self.assertEqual(export_csv([], buffer), 0)
        self.assertEqual(buffer.getvalue(), HEADER)
        buffer = io.StringIO()
        export_csv(iter([]), buffer, columns=["name"])
        self.assertEqual(buffer.getvalue(), "name\n")

    def test_accepts_generators(self):
        buffer = io.StringIO()
        self.assertEqual(export_csv((i for i in items()), buffer, columns=["sku"]), 3)
        self.assertEqual(buffer.getvalue(), "sku\nA-1\nB-2\nC-3\n")

    def test_unknown_column(self):
        buffer = io.StringIO()
        with self.assertRaises(ValueError):
            export_csv(items(), buffer, columns=["sku", "colour"])
        self.assertEqual(buffer.getvalue(), "")

    def test_write_to_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            for target in (Path(tmp) / "a.csv", str(Path(tmp) / "b.csv")):
                self.assertEqual(export_csv(load_items(SAMPLE), target), 3)
                self.assertEqual(Path(target).read_bytes(), SAMPLE_CSV.encode())
            missing = Path(tmp) / "never.csv"
            with self.assertRaises(ValueError):
                export_csv(items(), missing, columns=["nope"])
            self.assertFalse(missing.exists())

    def test_unicode(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "u.csv"
            export_csv([Item("Ü-1", "Crème brûlée", 1, 3.333, False, None)], path)
            self.assertEqual(path.read_text(encoding="utf-8"), HEADER + "Ü-1,Crème brûlée,1,3.33,no,\n")


class HiddenCliTest(unittest.TestCase):
    def run_cli(self, *args):
        return subprocess.run([sys.executable, str(HERE / "inventory.py"), *args],
                              capture_output=True, text=True, timeout=30)

    def test_csv_option(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "export.csv"
            result = self.run_cli(str(SAMPLE), "--csv", str(out))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, f"wrote 3 rows to {out}\n")
            self.assertEqual(out.read_bytes(), SAMPLE_CSV.encode())

    def test_table_without_option(self):
        result = self.run_cli(str(SAMPLE))
        self.assertEqual(result.returncode, 0)
        self.assertTrue(result.stdout.startswith("SKU "))


if __name__ == "__main__":
    unittest.main()
