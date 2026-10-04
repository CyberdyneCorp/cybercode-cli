import unittest
from pathlib import Path

from inventory import format_table, load_items

SAMPLE = Path(__file__).resolve().parent / "sample_items.json"


class InventoryTest(unittest.TestCase):
    def test_load_items(self):
        items = load_items(SAMPLE)
        self.assertEqual([i.sku for i in items], ["MUG-01", "TEA-02", "SPN-03"])
        self.assertIsNone(items[1].supplier)

    def test_format_table(self):
        table = format_table(load_items(SAMPLE)[:1])
        self.assertEqual(table, "SKU     NAME        QTY  PRICE\nMUG-01  Mug, large  12   7.50\n")


if __name__ == "__main__":
    unittest.main()
