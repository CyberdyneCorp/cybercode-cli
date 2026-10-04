import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from inventory.cli import main


class CliTestCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.db = Path(self.tmp.name) / "inventory.json"

    def run_cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = main(["--db", str(self.db), *args])
            except SystemExit as exit_:
                code = exit_.code
        return code, out.getvalue(), err.getvalue()

    def ok(self, *args):
        code, out, err = self.run_cli(*args)
        self.assertEqual(code, 0, err)
        return out

    def seed(self):
        self.ok("add", "A-100", "Hex key set", "--category", "tools", "--quantity", "4", "--price", "12.5")
        self.ok("add", "B-7", "Glue", "--quantity", "10", "--price", "3")
        self.ok("add", "C-1", "Saw", "--category", "tools", "--quantity", "1", "--price", "12.50")


class AddTest(CliTestCase):
    def test_add_writes_store(self):
        self.assertEqual(self.ok("add", "A-1", " Widget ", "--price", "2.5"), "added A-1\n")
        data = json.loads(self.db.read_text())
        self.assertEqual(data, {"version": 1, "items": [
            {"sku": "A-1", "name": "Widget", "category": "general", "quantity": 0, "price": "2.50"}]})

    def test_items_are_sorted_by_sku(self):
        self.ok("add", "Z-1", "Z", "--price", "1")
        self.ok("add", "A-1", "A", "--price", "1")
        skus = [i["sku"] for i in json.loads(self.db.read_text())["items"]]
        self.assertEqual(skus, ["A-1", "Z-1"])

    def test_duplicate_sku(self):
        self.ok("add", "A-1", "Widget", "--price", "1")
        self.assertEqual(self.run_cli("add", "A-1", "Other", "--price", "1"),
                         (1, "", "error: sku A-1 already exists\n"))

    def test_invalid_values(self):
        cases = [
            (["a-1", "x", "--price", "1"], "error: invalid sku: a-1\n"),
            (["A", "x", "--price", "1"], "error: invalid sku: A\n"),
            (["-A1", "x", "--price", "1"], None),
            (["A-1", "  ", "--price", "1"], "error: invalid name:   \n"),
            (["A-1", "x" * 61, "--price", "1"], f"error: invalid name: {'x' * 61}\n"),
            (["A-1", "x", "--category", "Tools", "--price", "1"], "error: invalid category: Tools\n"),
            (["A-1", "x", "--quantity", "-1", "--price", "1"], "error: invalid quantity: -1\n"),
            (["A-1", "x", "--quantity", "1.5", "--price", "1"], "error: invalid quantity: 1.5\n"),
            (["A-1", "x", "--price", "1.234"], "error: invalid price: 1.234\n"),
            (["A-1", "x", "--price", "abc"], "error: invalid price: abc\n"),
        ]
        for args, message in cases:
            with self.subTest(args=args):
                code, _, err = self.run_cli("add", *args)
                self.assertEqual(code, 2)
                if message:
                    self.assertEqual(err, message)
        self.assertFalse(self.db.exists())

    def test_missing_price_is_usage_error(self):
        self.assertEqual(self.run_cli("add", "A-1", "x")[0], 2)


class RemoveUpdateStockTest(CliTestCase):
    def test_remove(self):
        self.seed()
        self.assertEqual(self.ok("remove", "B-7"), "removed B-7\n")
        self.assertEqual(self.run_cli("remove", "B-7"), (1, "", "error: sku B-7 not found\n"))

    def test_update(self):
        self.seed()
        self.assertEqual(self.ok("update", "B-7", "--name", "Wood glue", "--price", "4"), "updated B-7\n")
        self.assertIn("name: Wood glue\n", self.ok("show", "B-7"))
        self.assertIn("price: 4.00\n", self.ok("show", "B-7"))

    def test_update_needs_a_field(self):
        self.seed()
        self.assertEqual(self.run_cli("update", "B-7"), (2, "", "error: nothing to update\n"))

    def test_update_unknown(self):
        self.assertEqual(self.run_cli("update", "X-1", "--name", "y"), (1, "", "error: sku X-1 not found\n"))

    def test_stock(self):
        self.seed()
        self.assertEqual(self.ok("stock", "C-1", "+5"), "C-1 quantity 6\n")
        self.assertEqual(self.ok("stock", "C-1", "-6"), "C-1 quantity 0\n")
        self.assertEqual(self.run_cli("stock", "C-1", "-1"),
                         (1, "", "error: insufficient stock for C-1 (have 0, need 1)\n"))
        self.assertEqual(self.run_cli("stock", "C-1", "x")[0:3:2], (2, "error: invalid delta: x\n"))


class ShowListReportTest(CliTestCase):
    def test_show(self):
        self.seed()
        self.assertEqual(self.ok("show", "A-100"), "sku: A-100\nname: Hex key set\ncategory: tools\n"
                                                   "quantity: 4\nprice: 12.50\nvalue: 50.00\n")
        self.assertEqual(self.run_cli("show", "ab")[0], 2)

    def test_list_table(self):
        self.seed()
        self.assertEqual(self.ok("list"), (
            "SKU    NAME         CATEGORY  QTY  PRICE  VALUE\n"
            "A-100  Hex key set  tools       4  12.50  50.00\n"
            "B-7    Glue         general    10   3.00  30.00\n"
            "C-1    Saw          tools       1  12.50  12.50\n"
            "3 items, total value 92.50\n"))

    def test_list_empty_does_not_create_store(self):
        self.assertEqual(self.ok("list"), "no items\n")
        self.assertFalse(self.db.exists())

    def test_list_filters_and_sorting(self):
        self.seed()
        rows = json.loads(self.ok("list", "--category", "tools", "--sort", "value", "--format", "json"))
        self.assertEqual([r["sku"] for r in rows], ["C-1", "A-100"])
        self.assertEqual(rows[0]["value"], "12.50")
        rows = json.loads(self.ok("list", "--low-stock", "4", "--reverse", "--format", "json"))
        self.assertEqual([r["sku"] for r in rows], ["C-1", "A-100"])
        rows = json.loads(self.ok("list", "--search", "glu", "--format", "json"))
        self.assertEqual([r["sku"] for r in rows], ["B-7"])
        self.assertEqual(self.ok("list", "--search", "nothing"), "no items\n")
        self.assertEqual(self.run_cli("list", "--low-stock", "x")[0:3:2], (2, "error: invalid quantity: x\n"))

    def test_report(self):
        self.seed()
        self.assertEqual(self.ok("report"), "general: 1 item, 10 units, value 30.00\n"
                                            "tools: 2 items, 5 units, value 62.50\n"
                                            "total: 3 items, 15 units, value 92.50\n")

    def test_empty_report(self):
        self.assertEqual(self.ok("report"), "total: 0 items, 0 units, value 0.00\n")


class StoreTest(CliTestCase):
    def test_corrupt_store_is_not_overwritten(self):
        self.db.write_text("{not json")
        self.assertEqual(self.run_cli("add", "A-1", "x", "--price", "1"),
                         (3, "", f"error: cannot read store {self.db}\n"))
        self.assertEqual(self.db.read_text(), "{not json")

    def test_wrong_shape_is_unreadable(self):
        self.db.write_text('{"version": 1, "items": [{"sku": "A-1"}]}')
        self.assertEqual(self.run_cli("list")[0], 3)

    def test_environment_variable_selects_store(self):
        path = Path(self.tmp.name) / "env" / "inv.json"
        with mock.patch.dict(os.environ, {"INVENTORY_DB": str(path)}), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(main(["add", "A-1", "x", "--price", "1"]), 0)
        self.assertTrue(path.exists())


if __name__ == "__main__":
    unittest.main()
