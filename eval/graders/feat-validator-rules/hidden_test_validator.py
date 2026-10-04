import unittest

from validator import validate

ORDER = {
    "type": "object",
    "required": ["id", "status", "lines"],
    "properties": {
        "id": {"type": "string"},
        "status": {"type": "string", "enum": ["new", "paid", "shipped"]},
        "priority": {"enum": [1, 2, 3]},
        "lines": {
            "type": "array",
            "items": {
                "type": "object",
                "required": ["sku", "quantity"],
                "properties": {
                    "sku": {"type": "string"},
                    "quantity": {"type": "integer", "minimum": 1},
                    "unit": {"enum": ["each", "box", None]},
                },
            },
        },
        "tags": {"type": "array", "items": {"type": "string"}},
    },
}


def order(**overrides):
    base = {"id": "o-1", "status": "new", "lines": [{"sku": "A", "quantity": 1}]}
    base.update(overrides)
    return base


class HiddenExistingRulesTest(unittest.TestCase):
    def test_visible_cases_still_hold(self):
        schema = {"type": "object", "required": ["id", "quantity"], "properties": {
            "id": {"type": "string"}, "quantity": {"type": "integer", "minimum": 1, "maximum": 99},
            "customer": {"type": "object", "required": ["email"]}}}
        self.assertEqual(validate(schema, {"id": "o-1", "quantity": 3}), [])
        self.assertEqual(validate(schema, []), ["$: expected object, got array"])
        self.assertEqual(validate(schema, {"id": 7, "quantity": 100, "customer": {}}), [
            "$.id: expected string, got integer",
            "$.quantity: must be <= 99",
            "$.customer: missing required property 'email'",
        ])


class HiddenEnumTest(unittest.TestCase):
    def test_valid(self):
        self.assertEqual(validate(ORDER, order(status="paid", priority=2)), [])

    def test_string_enum_message(self):
        self.assertEqual(validate(ORDER, order(status="lost")),
                         ['$.status: must be one of ["new", "paid", "shipped"]'])

    def test_number_enum_message(self):
        self.assertEqual(validate(ORDER, order(priority=5)), ["$.priority: must be one of [1, 2, 3]"])

    def test_enum_with_null(self):
        schema = {"enum": ["each", "box", None]}
        self.assertEqual(validate(schema, None), [])
        self.assertEqual(validate(schema, "kg"), ['$: must be one of ["each", "box", null]'])

    def test_enum_compares_json_types(self):
        self.assertEqual(validate({"enum": [1, "a"]}, 1.0), [])
        self.assertEqual(len(validate({"enum": [1]}, True)), 1)
        self.assertEqual(len(validate({"enum": [True]}, 1)), 1)
        self.assertEqual(len(validate({"enum": [0]}, False)), 1)
        self.assertEqual(validate({"enum": [False]}, False), [])

    def test_enum_is_case_sensitive(self):
        self.assertEqual(len(validate({"enum": ["new"]}, "NEW")), 1)

    def test_type_error_suppresses_enum(self):
        self.assertEqual(validate(ORDER, order(status=3)), ["$.status: expected string, got integer"])

    def test_enum_combines_with_minimum(self):
        errors = validate({"type": "integer", "enum": [5, 10], "minimum": 6}, 4)
        self.assertCountEqual(errors, ["$: must be one of [5, 10]", "$: must be >= 6"])


class HiddenItemsTest(unittest.TestCase):
    def test_each_item_validated_with_index_path(self):
        data = order(lines=[{"sku": "A", "quantity": 1}, {"sku": 9, "quantity": 0}, {"quantity": 2}])
        self.assertEqual(validate(ORDER, data), [
            "$.lines[1].sku: expected string, got integer",
            "$.lines[1].quantity: must be >= 1",
            "$.lines[2]: missing required property 'sku'",
        ])

    def test_items_with_enum(self):
        data = order(lines=[{"sku": "A", "quantity": 1, "unit": "box"}, {"sku": "B", "quantity": 1, "unit": "kg"}])
        self.assertEqual(validate(ORDER, data), ['$.lines[1].unit: must be one of ["each", "box", null]'])

    def test_scalar_items(self):
        self.assertEqual(validate(ORDER, order(tags=["a", 1, "b", None])), [
            "$.tags[1]: expected string, got integer",
            "$.tags[3]: expected string, got null",
        ])

    def test_empty_array_is_valid(self):
        self.assertEqual(validate(ORDER, order(lines=[])), [])

    def test_nested_arrays(self):
        schema = {"type": "array", "items": {"type": "array", "items": {"type": "number", "maximum": 1}}}
        self.assertEqual(validate(schema, [[0.5], [1, 2.5], []]), ["$[1][1]: must be <= 1"])

    def test_items_ignored_for_non_arrays(self):
        self.assertEqual(validate({"items": {"type": "string"}}, {"a": 1}), [])
        self.assertEqual(validate({"items": {"type": "string"}}, "abc"), [])

    def test_type_error_on_array_suppresses_items(self):
        self.assertEqual(validate(ORDER, order(lines="A")), ["$.lines: expected array, got string"])

    def test_missing_and_items_errors_together(self):
        data = {"id": "o-2", "lines": [{"sku": "A", "quantity": -1}]}
        self.assertEqual(validate(ORDER, data), [
            "$: missing required property 'status'",
            "$.lines[0].quantity: must be >= 1",
        ])


if __name__ == "__main__":
    unittest.main()
