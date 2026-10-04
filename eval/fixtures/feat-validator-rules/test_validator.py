import unittest

from validator import validate

ORDER = {
    "type": "object",
    "required": ["id", "quantity"],
    "properties": {
        "id": {"type": "string"},
        "quantity": {"type": "integer", "minimum": 1, "maximum": 99},
        "customer": {"type": "object", "required": ["email"]},
    },
}


class ValidatorTest(unittest.TestCase):
    def test_valid_order(self):
        self.assertEqual(validate(ORDER, {"id": "o-1", "quantity": 3}), [])

    def test_type_mismatch(self):
        self.assertEqual(validate(ORDER, []), ["$: expected object, got array"])

    def test_nested_paths(self):
        errors = validate(ORDER, {"id": 7, "quantity": 0, "customer": {}})
        self.assertEqual(errors, [
            "$.id: expected string, got integer",
            "$.quantity: must be >= 1",
            "$.customer: missing required property 'email'",
        ])


if __name__ == "__main__":
    unittest.main()
