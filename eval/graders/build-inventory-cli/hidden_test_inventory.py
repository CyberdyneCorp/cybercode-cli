"""Hidden behavior tests for build-inventory-cli. Drives `python3 -m inventory` via subprocess.

The package under test is found through the INVENTORY_WORKSPACE environment variable.
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

WORKSPACE = os.environ["INVENTORY_WORKSPACE"]
TIMEOUT = 30

SEED_TABLE = (
    "SKU    NAME         CATEGORY  QTY  PRICE  VALUE\n"
    "A-100  Hex key set  tools       4  12.50  50.00\n"
    "B-7    Glue         general    10   3.00  30.00\n"
    "C-1    Saw          tools       1  12.50  12.50\n"
    "3 items, total value 92.50\n"
)


class InventoryTestCase(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)
        self.db = self.dir / "data" / "inv.json"

    def cli(self, *args, db=True, env=None, cwd=None):
        command = [sys.executable, "-m", "inventory"] + (["--db", str(self.db)] if db else []) + list(args)
        environment = {k: v for k, v in os.environ.items() if k != "INVENTORY_DB"}
        environment.update(PYTHONPATH=WORKSPACE, PYTHONDONTWRITEBYTECODE="1", **(env or {}))
        return subprocess.run(command, cwd=cwd or self.dir, env=environment, capture_output=True,
                              text=True, timeout=TIMEOUT)

    def ok(self, *args, **kwargs) -> str:
        result = self.cli(*args, **kwargs)
        self.assertEqual(result.returncode, 0, f"{args}: {result.stderr}")
        self.assertEqual(result.stderr, "", f"{args} wrote to stderr")
        return result.stdout

    def fails(self, code: int, message: str | None, *args, **kwargs):
        result = self.cli(*args, **kwargs)
        self.assertEqual(result.returncode, code, f"{args}: {result.stdout}{result.stderr}")
        self.assertEqual(result.stdout, "", f"{args} wrote to stdout on error")
        if message is not None:
            self.assertEqual(result.stderr.strip(), message, f"{args}")
        return result

    def seed(self):
        self.ok("add", "A-100", "Hex key set", "--category", "tools", "--quantity", "4", "--price", "12.5")
        self.ok("add", "B-7", "Glue", "--quantity", "10", "--price", "3")
        self.ok("add", "C-1", "Saw", "--category", "tools", "--quantity", "1", "--price", "12.50")

    def store(self) -> dict:
        return json.loads(self.db.read_text(encoding="utf-8"))

    def skus(self, *args) -> list[str]:
        return [row["sku"] for row in json.loads(self.ok("list", "--format", "json", *args))]


class StoreFormatTest(InventoryTestCase):
    def test_add_creates_store_with_exact_shape(self):
        self.assertEqual(self.ok("add", "Z-9", "  Widget  ", "--price", "2.5"), "added Z-9\n")
        self.ok("add", "A-1", "Bolt", "--category", "hw-2", "--quantity", "007", "--price", "0")
        text = self.db.read_text(encoding="utf-8")
        data = json.loads(text)
        self.assertEqual(data, {"version": 1, "items": [
            {"sku": "A-1", "name": "Bolt", "category": "hw-2", "quantity": 7, "price": "0.00"},
            {"sku": "Z-9", "name": "Widget", "category": "general", "quantity": 0, "price": "2.50"},
        ]})
        self.assertEqual(text, json.dumps(data, indent=2) + "\n", "indent=2 with a trailing newline")

    def test_default_store_is_in_current_directory(self):
        self.ok("add", "A-1", "x", "--price", "1", db=False)
        self.assertTrue((self.dir / "inventory.json").exists())

    def test_environment_variable_and_precedence(self):
        env_db = self.dir / "env.json"
        self.ok("add", "E-1", "x", "--price", "1", db=False, env={"INVENTORY_DB": str(env_db)})
        self.assertTrue(env_db.exists())
        self.ok("add", "D-1", "x", "--price", "1", env={"INVENTORY_DB": str(env_db)})
        self.assertEqual([i["sku"] for i in self.store()["items"]], ["D-1"])
        self.assertNotIn("D-1", env_db.read_text())
        self.ok("add", "F-1", "x", "--price", "1", db=False, env={"INVENTORY_DB": ""})
        self.assertTrue((self.dir / "inventory.json").exists())

    def test_read_only_commands_do_not_create_store(self):
        self.assertEqual(self.ok("list"), "no items\n")
        self.assertEqual(self.ok("list", "--format", "json").strip(), "[]")
        self.assertEqual(self.ok("report"), "total: 0 items, 0 units, value 0.00\n")
        self.fails(1, "error: sku A-1 not found", "show", "A-1")
        self.assertFalse(self.db.exists())
        self.assertFalse(self.db.parent.exists())

    def test_unreadable_stores(self):
        for content in ["{not json", "[]", '{"version": 2, "items": []}', '{"version": 1, "items": {}}',
                        '{"version": 1, "items": [{"sku": "A-1"}]}',
                        '{"version": 1, "items": [{"sku": "A-1", "name": "x", "category": "general", '
                        '"quantity": "1", "price": "1.00"}]}']:
            with self.subTest(content=content):
                self.db.parent.mkdir(parents=True, exist_ok=True)
                self.db.write_text(content)
                message = f"error: cannot read store {self.db}"
                self.fails(3, message, "list")
                self.fails(3, message, "report")
                self.fails(3, message, "add", "A-2", "x", "--price", "1")
                self.fails(3, message, "stock", "A-1", "1")
                self.assertEqual(self.db.read_text(), content)


class AddTest(InventoryTestCase):
    def test_duplicate(self):
        self.seed()
        before = self.db.read_bytes()
        self.fails(1, "error: sku B-7 already exists", "add", "B-7", "Other", "--price", "1")
        self.assertEqual(self.db.read_bytes(), before)

    def test_invalid_values(self):
        cases = [
            ("error: invalid sku: a-1", ["a-1", "x", "--price", "1"]),
            ("error: invalid sku: A", ["A", "x", "--price", "1"]),
            ("error: invalid sku: A_1", ["A_1", "x", "--price", "1"]),
            ("error: invalid sku: " + "A" * 21, ["A" * 21, "x", "--price", "1"]),
            ("error: invalid name: " + "n" * 61, ["A-1", "n" * 61, "--price", "1"]),
            ("error: invalid category: Tools", ["A-1", "x", "--category", "Tools", "--price", "1"]),
            ("error: invalid category: " + "c" * 21, ["A-1", "x", "--category", "c" * 21, "--price", "1"]),
            ("error: invalid quantity: 1.5", ["A-1", "x", "--quantity", "1.5", "--price", "1"]),
            ("error: invalid quantity: -2", ["A-1", "x", "--quantity", "-2", "--price", "1"]),
            ("error: invalid quantity: ten", ["A-1", "x", "--quantity", "ten", "--price", "1"]),
            ("error: invalid price: 1.234", ["A-1", "x", "--price", "1.234"]),
            ("error: invalid price: -1", ["A-1", "x", "--price", "-1"]),
            ("error: invalid price: 1.", ["A-1", "x", "--price", "1."]),
            ("error: invalid price: abc", ["A-1", "x", "--price", "abc"]),
            ("error: invalid price: 1e3", ["A-1", "x", "--price", "1e3"]),
        ]
        for message, args in cases:
            with self.subTest(args=args):
                self.fails(2, message, "add", *args)
        self.assertFalse(self.db.exists())

    def test_blank_name_is_invalid(self):
        result = self.fails(2, None, "add", "A-1", "   ", "--price", "1")
        self.assertTrue(result.stderr.startswith("error: invalid name"), result.stderr)

    def test_boundary_values_are_valid(self):
        self.ok("add", "A1", "n" * 60, "--price", "0.5")
        self.ok("add", "9" + "-" * 19, "y", "--category", "c" * 20, "--price", "10")
        self.assertEqual([i["price"] for i in self.store()["items"]], ["10.00", "0.50"])

    def test_usage_errors(self):
        self.fails(2, None, "add", "A-1", "x")
        self.fails(2, None, "add", "A-1")
        self.fails(2, None, "frobnicate")
        self.fails(2, None)


class ChangeCommandsTest(InventoryTestCase):
    def test_remove(self):
        self.seed()
        self.assertEqual(self.ok("remove", "B-7"), "removed B-7\n")
        self.assertEqual([i["sku"] for i in self.store()["items"]], ["A-100", "C-1"])
        self.fails(1, "error: sku B-7 not found", "remove", "B-7")
        self.fails(2, "error: invalid sku: b-7", "remove", "b-7")

    def test_update(self):
        self.seed()
        self.assertEqual(self.ok("update", "B-7", "--name", " Wood glue ", "--price", "4.5"), "updated B-7\n")
        self.ok("update", "C-1", "--category", "saws")
        items = {i["sku"]: i for i in self.store()["items"]}
        self.assertEqual(items["B-7"], {"sku": "B-7", "name": "Wood glue", "category": "general",
                                        "quantity": 10, "price": "4.50"})
        self.assertEqual(items["C-1"]["category"], "saws")
        self.assertEqual(items["C-1"]["price"], "12.50")

    def test_update_errors(self):
        self.seed()
        before = self.db.read_bytes()
        self.fails(2, "error: nothing to update", "update", "B-7")
        self.fails(1, "error: sku X-1 not found", "update", "X-1", "--name", "y")
        self.fails(2, "error: invalid price: 4.555", "update", "B-7", "--price", "4.555")
        self.fails(2, "error: invalid category: A", "update", "B-7", "--category", "A")
        self.assertEqual(self.db.read_bytes(), before)

    def test_stock(self):
        self.seed()
        self.assertEqual(self.ok("stock", "C-1", "+5"), "C-1 quantity 6\n")
        self.assertEqual(self.ok("stock", "C-1", "-2"), "C-1 quantity 4\n")
        self.assertEqual(self.ok("stock", "C-1", "0"), "C-1 quantity 4\n")
        self.assertEqual(self.ok("stock", "C-1", "3"), "C-1 quantity 7\n")
        self.assertEqual(self.ok("stock", "C-1", "-7"), "C-1 quantity 0\n")
        self.assertEqual({i["sku"]: i["quantity"] for i in self.store()["items"]}["C-1"], 0)

    def test_stock_errors(self):
        self.seed()
        before = self.db.read_bytes()
        self.fails(1, "error: insufficient stock for B-7 (have 10, need 11)", "stock", "B-7", "-11")
        self.fails(1, "error: sku Q-1 not found", "stock", "Q-1", "1")
        self.fails(2, "error: invalid delta: x", "stock", "B-7", "x")
        self.fails(2, "error: invalid delta: 1.5", "stock", "B-7", "1.5")
        self.assertEqual(self.db.read_bytes(), before)


class ReadCommandsTest(InventoryTestCase):
    def test_show(self):
        self.seed()
        self.assertEqual(self.ok("show", "A-100"), "sku: A-100\nname: Hex key set\ncategory: tools\n"
                                                   "quantity: 4\nprice: 12.50\nvalue: 50.00\n")
        self.fails(2, "error: invalid sku: ab", "show", "ab")

    def test_list_table(self):
        self.seed()
        self.assertEqual(self.ok("list"), SEED_TABLE)

    def test_table_widths_and_singular(self):
        self.ok("add", "LONG-SKU-0001", "A rather long item name", "--category", "x",
                "--quantity", "12345", "--price", "1234.5")
        self.assertEqual(self.ok("list"), (
            "SKU            NAME                     CATEGORY    QTY    PRICE        VALUE\n"
            "LONG-SKU-0001  A rather long item name  x         12345  1234.50  15239902.50\n"
            "1 item, total value 15239902.50\n"))

    def test_no_trailing_whitespace(self):
        self.seed()
        self.ok("add", "D-1", "Zz", "--category", "a", "--price", "1")
        for line in self.ok("list", "--sort", "name").splitlines():
            self.assertEqual(line, line.rstrip())

    def test_filters(self):
        self.seed()
        self.ok("add", "D-1", "Saw blade", "--category", "saws", "--quantity", "3", "--price", "2")
        self.assertEqual(self.skus("--category", "tools"), ["A-100", "C-1"])
        self.assertEqual(self.skus("--search", "SAW"), ["C-1", "D-1"])
        self.assertEqual(self.skus("--search", "a-1"), ["A-100"])
        self.assertEqual(self.skus("--low-stock", "3"), ["C-1", "D-1"])
        self.assertEqual(self.skus("--low-stock", "0"), [])
        self.assertEqual(self.skus("--category", "tools", "--search", "s", "--low-stock", "3"), ["C-1"])
        self.assertEqual(self.ok("list", "--category", "nothing"), "no items\n")
        self.fails(2, "error: invalid quantity: x", "list", "--low-stock", "x")
        self.fails(2, "error: invalid category: Tools", "list", "--category", "Tools")

    def test_sorting(self):
        self.seed()
        self.ok("add", "D-1", "glue stick", "--quantity", "4", "--price", "12.5")
        self.assertEqual(self.skus(), ["A-100", "B-7", "C-1", "D-1"])
        self.assertEqual(self.skus("--sort", "name"), ["B-7", "D-1", "A-100", "C-1"])
        self.assertEqual(self.skus("--sort", "quantity"), ["C-1", "A-100", "D-1", "B-7"])
        self.assertEqual(self.skus("--sort", "value"), ["C-1", "B-7", "A-100", "D-1"])
        self.assertEqual(self.skus("--sort", "value", "--reverse"), ["D-1", "A-100", "B-7", "C-1"])
        self.assertEqual(self.skus("--reverse"), ["D-1", "C-1", "B-7", "A-100"])
        self.fails(2, None, "list", "--sort", "price")

    def test_json_format(self):
        self.seed()
        rows = json.loads(self.ok("list", "--format", "json", "--sort", "value"))
        self.assertEqual(rows[0], {"sku": "C-1", "name": "Saw", "category": "tools", "quantity": 1,
                                   "price": "12.50", "value": "12.50"})
        self.assertEqual([r["value"] for r in rows], ["12.50", "30.00", "50.00"])

    def test_report(self):
        self.seed()
        self.assertEqual(self.ok("report"), "general: 1 item, 10 units, value 30.00\n"
                                            "tools: 2 items, 5 units, value 62.50\n"
                                            "total: 3 items, 15 units, value 92.50\n")
        self.ok("stock", "B-7", "-9")
        self.assertTrue(self.ok("report").startswith("general: 1 item, 1 unit, value 3.00\n"))

    def test_money_is_exact(self):
        self.ok("add", "M-1", "dime", "--quantity", "3", "--price", "0.1")
        self.ok("add", "M-2", "penny", "--quantity", "7", "--price", "0.01")
        self.ok("add", "M-3", "odd", "--quantity", "3", "--price", "19.99")
        self.assertIn("value: 0.30\n", self.ok("show", "M-1"))
        self.assertEqual(self.ok("list").splitlines()[-1], "3 items, total value 60.34")
        self.assertEqual(self.ok("report").splitlines()[-1], "total: 3 items, 13 units, value 60.34")


if __name__ == "__main__":
    unittest.main()
